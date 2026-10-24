//! `get_next` (§332–§365) over the flat input stack ([`crate::input`]):
//! the token path of one run. The fast path is one bounds test on the
//! level on top; everything else is behind `#[inline(never)]`.
//!
//! What a command found as its input is marked in O(1) (the levels are
//! four words each, copied) so a stopped command can be rewound and the
//! run parked ([`Run::begin_command`], [`Run::rewind`]); no hashing
//! per command.

use crate::exec::LockWait as _;
use crate::input::{self, Level, Range, Seg, kind, ty};
use crate::input::{MID_LINE, NEW_LINE, SKIP_BLANKS};
use crate::run::{Cur, Halt, R, Run};
use crate::val::{Tok, cmd};

/// The commands an alignment entry watches for (§342, §347, §357):
/// braces, `&` and `\cr`, as a bit mask by command code.
const ALIGN_CMDS: u128 =
    1 << cmd::LEFT_BRACE | 1 << cmd::RIGHT_BRACE | 1 << cmd::TAB_MARK | 1 << cmd::CAR_RET;

impl Run<'_, '_> {
    // --------------------------------------------------------- get_next

    /// `get_next` with the meaning (§357–§358): false at the end of all
    /// input. No `Result` (spec-final §2.2): a halt is raised
    /// ([`Run::raise`]) and the current token is the sentinel; the
    /// command loop takes it once a command ([`Run::ck`]).
    // the per-token hot path: inlined on purpose
    #[allow(clippy::inline_always)]
    #[inline(always)]
    pub(crate) fn get_next(&mut self) -> bool {
        self.fetch()
    }

    /// The halt raised on the token path, if any, as a `Result`: what a
    /// command's scanners check where they go on only with a token.
    #[allow(clippy::inline_always)]
    #[inline(always)]
    pub(crate) fn ck(&mut self) -> R<()> {
        match self.halt.take() {
            None => Ok(()),
            Some(h) => Err(h),
        }
    }

    /// [`Run::get_next`] and [`Run::ck`]: for the commands that read a
    /// token and stop on a halt.
    #[inline]
    pub(crate) fn next_ck(&mut self) -> R<bool> {
        let b = self.get_next();
        self.ck()?;
        Ok(b)
    }

    /// `get_next` without a `Result`: a halt is raised ([`Run::raise`]).
    #[allow(clippy::inline_always)]
    #[inline(always)]
    pub(crate) fn fetch(&mut self) -> bool {
        // (the common case: a token of the list on top that is not an
        // internal marker, and no alignment entry to watch for)
        if let Some(tok) = self.input.next_plain_of_top() {
            self.set_cur_flag(tok);
            if self.cur.cmd >= cmd::OUTER_CALL
                && self.cur.cmd <= cmd::LONG_OUTER_CALL
                && (self.scanner.is_some() || self.in_skip)
            {
                if let Err(h) = self.outer_validity() {
                    self.raise(h);
                }
                return true;
            }
            if self.aopen != 0
                && ALIGN_CMDS.checked_shr(u32::from(self.cur.cmd)).unwrap_or(0) & 1 != 0
            {
                match self.align_token() {
                    Ok(true) => return self.fetch_slow(),
                    Ok(false) => {}
                    Err(h) => self.raise(h),
                }
            }
            return true;
        }
        self.fetch_slow()
    }

    /// The main loop's `get_next` (§343–§344, §347–§350) for the line on
    /// top, already reached: a letter or other character next (by the
    /// catcodes this run has cached) goes into `cur`, as does a space in
    /// state `mid_line` (§348's spacer; blanks in another state are
    /// skipped, §350), and the answer is true. Anything else (the line's
    /// end, a control sequence, a catcode not cached) is the slow path's:
    /// false, with nothing but skipped blanks read.
    #[allow(clippy::inline_always)]
    #[inline(always)]
    pub(crate) fn fast_char(&mut self) -> bool {
        if self.cat_gen != self.st.cat_gen() {
            return false;
        }
        let Some(l) = self.input.levels.last() else {
            return false;
        };
        if l.kind != input::kind::LINE {
            return false;
        }
        let line = &mut self.input.lines[l.a as usize];
        if line.endch == -2 {
            return false;
        }
        loop {
            let Some(&c) = line.buf.get(line.loc) else {
                return false;
            };
            if c == 0 || c > 255 {
                return false;
            }
            let cat = self.cat_cache[c as usize];
            match cat {
                cmd::LETTER | cmd::OTHER_CHAR => {
                    line.loc += 1;
                    line.state = MID_LINE;
                    self.cur = Cur {
                        tok: Tok::ch(cat, c),
                        cmd: cat,
                        chr: c.cast_signed(),
                    };
                    return true;
                }
                cmd::SPACER => {
                    line.loc += 1;
                    if line.state == MID_LINE {
                        line.state = SKIP_BLANKS;
                        self.cur = Cur {
                            tok: Tok::ch(cmd::SPACER, 32),
                            cmd: cmd::SPACER,
                            chr: 32,
                        };
                        return true;
                    }
                }
                _ => return false,
            }
        }
    }

    #[inline(never)]
    fn fetch_slow(&mut self) -> bool {
        match self.get_next_slow() {
            Ok(b) => b,
            Err(h) => {
                self.raise(h);
                true
            }
        }
    }

    #[inline(never)]
    fn get_next_slow(&mut self) -> R<bool> {
        loop {
            let t = loop {
                let Some(t) = self.next_tok()? else {
                    return Ok(false);
                };
                if let Some(n) = t.as_cs()
                    && n.frozen()
                {
                    if n.starts_with("\u{10FFFE}eof") {
                        // §362: the end of an \input file (or a pseudo file)
                        crate::etex::end_of_file(self, n.name())?;
                        continue;
                    }
                    if n.name() == "\u{10FFFE}uend" {
                        // §324: the end of a u template
                        self.align_u_end();
                        continue;
                    }
                }
                break t;
            };
            if let Some(n) = t.as_cs()
                && n.frozen()
                && let Some(k) = n.strip_prefix("\u{10FFFE}endv:")
            {
                // §789: the end of a v template (`endv`, §380)
                let chr = k.parse().unwrap_or(0);
                self.cur = Cur {
                    tok: t,
                    cmd: cmd::IGNORE,
                    chr,
                };
                return Ok(true);
            }
            if t == crate::expand::notexpanded_tok() {
                // §358
                let Some(t2) = self.next_tok()? else {
                    return Ok(false);
                };
                self.set_cur(t2)?;
                if self.cur.cmd > cmd::MAX_COMMAND {
                    self.cur.cmd = cmd::RELAX;
                    self.cur.chr = crate::val::NO_EXPAND_FLAG;
                }
                return Ok(true);
            }
            self.set_cur(t)?;
            if self.cur.cmd >= cmd::OUTER_CALL
                && self.cur.cmd <= cmd::LONG_OUTER_CALL
                && (self.scanner.is_some() || self.in_skip)
            {
                self.outer_validity()?;
                return Ok(true);
            }
            if self.aopen != 0
                && matches!(
                    self.cur.cmd,
                    cmd::LEFT_BRACE | cmd::RIGHT_BRACE | cmd::TAB_MARK | cmd::CAR_RET
                )
                && self.align_token()?
            {
                continue;
            }
            return Ok(true);
        }
    }

    /// `get_next`, at the end of all input a `\relax` (the end of a
    /// document without `\end` is the client's to report). A halt is
    /// raised, never returned.
    #[inline]
    pub(crate) fn next_or_end(&mut self) {
        if self.get_next() || self.halt.is_some() {
            if crate::exec::obs_flag!("PHITEX_CMD_TRACE") {
                self.tok_trace("next");
            }
            return;
        }
        self.at_end_of_input();
    }

    #[inline(never)]
    fn at_end_of_input(&mut self) {
        if self.eof {
            if self.interp.is_some() {
                // §362: the file ends; §336 checks what it ended in
                if self.interp_eof {
                    match self.file_ended() {
                        Ok(true) => return self.next_or_end(),
                        Ok(false) => {}
                        Err(h) => return self.raise(h),
                    }
                }
                self.interp_short = true;
            }
            return self.raise(Halt::Fatal);
        }
        self.cur = Cur {
            tok: crate::expand::relax_tok(),
            cmd: cmd::RELAX,
            chr: 256,
        };
    }

    /// [`Run::next_or_end`] and [`Run::ck`].
    #[inline]
    pub(crate) fn get_next_or_end(&mut self) -> R<()> {
        self.next_or_end();
        self.ck()
    }

    /// Where a line is now, for a message that names it later: the
    /// line's own number where it has one, else this run's lines begun
    /// so far ([`Run::input_line_no`] reads the rest when needed).
    pub(crate) fn line_mark(&self) -> (Option<i64>, i64) {
        match self.file_line_here() {
            Some(crate::run::FileLine::Known(n)) => (Some(n), 0),
            Some(crate::run::FileLine::Piece(name)) => (
                None,
                self.doc_lines
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map_or(0, |&(_, k)| k),
            ),
            None => (Some(0), 0),
        }
    }

    /// §336 `check_outer_validity`: an `\outer` macro (`cur`) met while
    /// a token list is being scanned or conditional text skipped. It is
    /// backed up to be read again and a space read in its place; a list
    /// being scanned is a runaway, "Forbidden control sequence found
    /// while scanning ... of \x", ended by what §339 inserts (`}` for a
    /// definition or a text, `\par` for an argument, which then ends
    /// without "Paragraph ended"); a skip is "Incomplete \if...; all
    /// text was ignored after line n", ended by an inserted `\fi`.
    #[cold]
    #[inline(never)]
    pub(crate) fn outer_validity(&mut self) -> R<()> {
        let t = self.cur.tok;
        self.back_input(t);
        self.cur = Cur {
            tok: Tok::ch(10, u32::from(b' ')),
            cmd: cmd::SPACER,
            chr: i32::from(b' '),
        };
        if !self.in_skip
            && let Some((status, w)) = self.scanner
        {
            // §338-§339
            let (what, ins) = match status {
                crate::run::SCAN_DEFINING => ("definition", Tok::ch(2, u32::from(b'}'))),
                crate::run::SCAN_MATCHING => {
                    self.outer_call = true;
                    ("use", crate::expand::par_tok())
                }
                _ => ("text", Tok::ch(2, u32::from(b'}'))),
            };
            let mut cs = String::new();
            self.show_toks_into(&mut cs, std::slice::from_ref(&w))?;
            let msg = format!(
                "Forbidden control sequence found while scanning {what} of {}",
                cs.trim_end()
            );
            self.outer_error(msg)?;
            self.push_toks([ins]);
            return Ok(());
        }
        // §336's skipping case
        let code = self.if_codes.last().copied().unwrap_or(0);
        let name = crate::prims::PRIMS
            .iter()
            .find(|&&(_, c, h)| c == cmd::IF_TEST && h == code)
            .map_or("if", |&(n, _, _)| n);
        let line = match self.skip_from {
            (Some(n), _) => n,
            (None, mine) => {
                let now = self.line_mark().1;
                i64::from(self.input_line_no()?) - (now - mine)
            }
        };
        let msg = format!("Incomplete \\{name}; all text was ignored after line {line}");
        self.outer_error(msg)?;
        self.push_toks([crate::expand::frozen_fi_tok()]);
        Ok(())
    }

    fn outer_error(&mut self, msg: String) -> R<()> {
        if self.interp.is_some() {
            self.interp_errs.push(msg);
            Ok(())
        } else {
            self.log("error", &msg)
        }
    }

    /// §336–§339 at the end of a file (§362), or of an interpreted one: a token list being
    /// scanned is a runaway, "File ended while scanning ... of \\x", and
    /// what ends it is inserted (`}` for a definition or a text, `\\par`
    /// for an argument, which then ends without "Paragraph ended",
    /// §392). False if nothing was being scanned (`normal`: no error).
    pub(crate) fn file_ended(&mut self) -> R<bool> {
        let Some((status, w)) = self.scanner else {
            return Ok(false);
        };
        let (what, ins) = match status {
            crate::run::SCAN_DEFINING => ("definition", Tok::ch(2, u32::from(b'}'))),
            crate::run::SCAN_MATCHING => {
                self.outer_call = true;
                ("use", crate::expand::par_tok())
            }
            _ => ("text", Tok::ch(2, u32::from(b'}'))),
        };
        let mut cs = String::new();
        self.show_toks_into(&mut cs, std::slice::from_ref(&w))?;
        let msg = format!("File ended while scanning {what} of {}", cs.trim_end());
        if self.interp.is_some() {
            // (a written file's text read as its read-back will read it:
            // the error is the read-back's, [`Run::read_back`])
            self.interp_errs.push(msg);
        } else {
            self.log("error", &msg)?;
        }
        self.push_toks([ins]);
        Ok(true)
    }

    /// The token `get_r_token` reads (§1215): a control sequence's
    /// meaning is not looked up unless an alignment of this run watches
    /// for `&` and `\cr` (§342); `\outer` is not checked here (§336
    /// at `normal`).
    pub(crate) fn get_name_token(&mut self) -> R<()> {
        self.bare = self.aopen <= 0;
        self.get_token();
        self.bare = false;
        self.ck()
    }

    /// `get_token` (§365): a halt is raised, never returned.
    #[inline]
    pub(crate) fn get_token(&mut self) {
        self.next_or_end();
    }

    /// `get_token`'s token alone (§365 for a macro's arguments, §392-§399:
    /// the meaning is not needed, every test there is on `cur_tok`); the
    /// internal markers still go through the slow path. A halt is raised
    /// (the sentinel returned).
    #[inline]
    pub(crate) fn get_token_tok(&mut self) -> Tok {
        // (an alignment of this run counts every brace, §342: the slow
        // path then)
        if self.aopen == 0
            && let Some(t) = self.input.next_plain_of_top()
        {
            self.cur.tok = t;
            return t;
        }
        self.get_token_stored();
        self.cur.tok
    }

    /// `get_token` for a token that is stored or matched, never expanded
    /// (a macro's arguments, a definition's parameter text and body, a
    /// token list's body, §392, §473–§479): of a control sequence's
    /// meaning only its class is read ([`Run::step_lookup`]).
    pub(crate) fn get_token_stored(&mut self) {
        let was = std::mem::replace(&mut self.step_reads, true);
        self.next_or_end();
        self.step_reads = was;
    }

    /// `get_x_token` (§380): a halt is raised, never returned.
    pub(crate) fn get_x_token(&mut self) {
        loop {
            self.next_or_end();
            if self.cur.cmd <= cmd::MAX_COMMAND || self.halt.is_some() {
                return;
            }
            // (a macro: its body goes on top here, no expansion level)
            if (cmd::CALL..cmd::END_TEMPLATE).contains(&self.cur.cmd) {
                // (a number scan's first token, [`Run::num_reads`]: what
                // a macro's arguments are made of is read in full)
                let nr = std::mem::replace(&mut self.num_reads, false);
                self.macro_call();
                self.num_reads = nr;
                if self.halt.is_some() {
                    return;
                }
                continue;
            }
            if self.cur.cmd >= cmd::END_TEMPLATE && self.cur.cmd != cmd::DONT_EXPAND {
                return;
            }
            let nr = std::mem::replace(&mut self.num_reads, false);
            self.expand();
            self.num_reads = nr;
            if self.halt.is_some() {
                return;
            }
        }
    }

    /// e-TeX's `get_x_or_protected`: [`Run::get_x_token`], a `\protected`
    /// macro left as it is, unexpanded (the first token of an alignment's
    /// entry, §785 and §791: such a macro expands after the u template
    /// is inserted, its braces counted in the entry's `align_state`).
    pub(crate) fn get_x_or_protected(&mut self) -> R<()> {
        loop {
            self.next_or_end();
            if self.cur.cmd <= cmd::MAX_COMMAND || self.halt.is_some() {
                return Ok(());
            }
            if (cmd::CALL..cmd::END_TEMPLATE).contains(&self.cur.cmd) {
                if matches!(&self.cur_mean()?, crate::val::Mean::Macro(x) if x.flags & crate::etex::PROTECTED != 0)
                {
                    return Ok(());
                }
                let nr = std::mem::replace(&mut self.num_reads, false);
                self.macro_call();
                self.num_reads = nr;
                if self.halt.is_some() {
                    return Ok(());
                }
                continue;
            }
            if self.cur.cmd >= cmd::END_TEMPLATE && self.cur.cmd != cmd::DONT_EXPAND {
                return Ok(());
            }
            let nr = std::mem::replace(&mut self.num_reads, false);
            self.expand();
            self.num_reads = nr;
            if self.halt.is_some() {
                return Ok(());
            }
        }
    }

    /// (observe) `PHITEX_TOK_TRACE=<a>[-<b>]`: in the node
    /// `PHITEX_CMD_TRACE` names, each token its commands `a` to `b` get, with its meaning's
    /// `cmd`/`chr` and the input levels.
    #[cold]
    #[inline(never)]
    pub(crate) fn tok_trace(&self, at: &str) {
        let Some((a, b)) = crate::exec::env_str!("PHITEX_TOK_TRACE").and_then(|x| {
            let (a, b) = x.split_once('-').unwrap_or((x, x));
            Some((a.parse::<u64>().ok()?, b.parse::<u64>().ok()?))
        }) else {
            return;
        };
        if !(a..=b).contains(&self.cmds) || !self.cmd_traced() {
            return;
        }
        let levels: Vec<(u8, u8, u32, u32)> = self
            .input
            .levels
            .iter()
            .map(|l| (l.kind, l.ty, l.c, l.b))
            .collect();
        let slot = crate::fields::mean_loc(self.cur.tok)
            .and_then(|l| crate::runstate::Loc::try_from(l).ok())
            .map(|l| format!("{} {}", self.st.debug_slot(l), self.st.debug_marks(l)))
            .unwrap_or_default();
        eprintln!(
            "tok-trace {at}: {:?} cmd {} chr {} halt {} levels {levels:?} slot {slot}",
            self.cur.tok,
            self.cur.cmd,
            self.cur.chr,
            self.halt.is_some()
        );
    }

    /// `x_token` (§381): a halt is raised, never returned.
    pub(crate) fn x_token(&mut self) {
        while self.cur.cmd > cmd::MAX_COMMAND && self.halt.is_none() {
            self.expand();
            if self.halt.is_some() {
                return;
            }
            self.next_or_end();
        }
    }

    // ------------------------------------------------------- the levels

    /// `get_next` (§341): the next token, its meaning not yet looked up.
    /// `None` at the end of all input.
    #[inline]
    pub(crate) fn next_tok(&mut self) -> R<Option<Tok>> {
        if let Some(t) = self.input.next_of_top() {
            return Ok(Some(t));
        }
        self.next_tok_slow()
    }

    #[inline(never)]
    fn next_tok_slow(&mut self) -> R<Option<Tok>> {
        loop {
            let Some(&top) = self.input.top() else {
                if self.input.reach_text() {
                    if self.pull_at.last() == Some(&Some(self.input.text_at - 1)) {
                        self.pull_lev = Some(self.input.levels.len() - 1);
                    }
                    continue;
                }
                if self.pull()? {
                    continue;
                }
                return Ok(None);
            };
            match top.kind {
                kind::TOKS => {
                    if top.a < top.b {
                        return Ok(self.input.next_of_top());
                    }
                    self.pop_input_level();
                }
                kind::LINE => {
                    if let Some(t) = self.src_next()? {
                        return Ok(Some(t));
                    }
                    self.pop_input_level();
                }
                _ => {
                    // (a document file's lines are paragraph-region
                    // children, [`Run::leftover`], at a command's start:
                    // [`Run::has_input`] stops the run there; a command
                    // still scanning, `\count1=\input n`, reads on into
                    // the file, §362)
                    if self.input.file(top).lazy() {
                        self.load_written(top)?;
                    }
                    let f = self.input.file_mut(top);
                    if f.has_lines() {
                        let line = f.take_line();
                        self.input.push_line(line);
                    } else {
                        self.pop_input_level();
                    }
                }
            }
        }
    }

    /// Pops the level on top, keeping what the command in progress found
    /// for [`Run::rewind`] (the mark holds the levels; nothing to keep).
    #[inline]
    fn pop_input_level(&mut self) {
        self.input.pop();
    }

    /// Pushes a token list on top (§323).
    #[inline]
    pub(crate) fn push_range(&mut self, r: Range, t: u8) {
        self.input.push_toks(r, t);
    }

    /// Pushes a token list copied into the arena (§323).
    #[inline]
    pub(crate) fn push_toks<T: AsRef<[Tok]>>(&mut self, toks: T) {
        let toks = toks.as_ref();
        if !toks.is_empty() {
            let r = self.input.arena.alloc(toks);
            self.input.push_toks(r, ty::OTHER);
        }
    }

    /// The arena range of a macro body (§390: read in place).
    pub(crate) fn body_range(&mut self, body: &std::rc::Rc<[Tok]>) -> Range {
        self.input.arena.body(body)
    }

    /// `back_input` (§325).
    #[inline]
    pub(crate) fn back_input(&mut self, t: Tok) {
        // §325: a brace put back is counted again when read
        if self.aopen != 0
            && let Some(s) = self.astate
            && !t.is_cs_or_active()
        {
            match t.cat() {
                Some(1) => self.set_astate(s - 1),
                Some(2) => self.set_astate(s + 1),
                _ => {}
            }
        }
        self.input.back(t);
    }

    // --------------------------------------------------------- the text

    /// Claims the next sibling's text (its frames go to the back of the
    /// text).
    pub(crate) fn pull(&mut self) -> R<bool> {
        if self.eof || self.no_pull {
            return Ok(false);
        }
        match self.cx.following(self.pulled) {
            Some((op, args)) if op == self.ops.exec => {
                let args = self.resolve_units(&args)?;
                let segs = crate::run::decode_segs(self.v, &args);
                // (where the sibling's text begins, unless its first
                // segment continues the line before it)
                let joins = self.input.text.len() > self.input.text_at
                    && matches!(
                        (self.input.text.last(), segs.first()),
                        (Some(Seg::Line(last)), Some(Seg::Line(next))) if !last.ends && last.endch == -2 && !next.starts
                    );
                if self.pull_at.len() == self.pulled {
                    self.pull_at.push((!joins).then_some(self.input.text.len()));
                    self.pull_orig.push(None);
                    let in_box = self.nest.iter().any(|l| l.owns && l.group != 0);
                    self.pull_box.push(in_box);
                    self.pull_lev = None;
                }
                if crate::exec::obs_flag!("PHITEX_GIVEBACK_TRACE") && self.doc {
                    let first: String = args
                        .first()
                        .map(|&x| self.v.render(x))
                        .unwrap_or_default()
                        .chars()
                        .take(60)
                        .collect();
                    eprintln!(
                        "pull: node {} pulls sibling {}: {first}",
                        self.cx.node().raw(),
                        self.pulled
                    );
                }
                self.muts += 1;
                self.pulled += 1;
                self.input.append_text(segs);
                Ok(true)
            }
            other => {
                if crate::exec::obs_flag!("PHITEX_TEX_TRACE") {
                    eprintln!(
                        "  pull {} stops: sibling {} is {:?}",
                        self.cx.node().raw(),
                        self.pulled,
                        other.map(|(op, _)| op)
                    );
                }
                if self.top {
                    self.muts += 1;
                    self.eof = true;
                    Ok(false)
                } else {
                    Err(Halt::Starve)
                }
            }
        }
    }

    /// At a command's start: when all that is left is the last sibling
    /// pulled, of which only its first token was read and put back
    /// (`\futurelet`, `\@ifnextchar` looking for an optional argument
    /// after a load), and that token is what the sibling's text gives
    /// under the codes in force now, the sibling is given back: unclaimed,
    /// it runs as its own node on the state this run leaves. True when
    /// the run is to end here. (A preamble's loads each end their run so,
    /// instead of every later load joining the first.)
    #[allow(clippy::too_many_lines)]
    pub(crate) fn give_back_peeked(&mut self) -> R<bool> {
        // (`PHITEX_GIVEBACK_TRACE`: a load's run, why a sibling it pulled
        // was not given back)
        // (`=all`: every run's)
        static ALL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let all =
            *ALL.get_or_init(|| std::env::var("PHITEX_GIVEBACK_TRACE").is_ok_and(|x| x == "all"));
        let trace = crate::exec::obs_flag!("PHITEX_GIVEBACK_TRACE") && (self.rec_load || all);
        let pulled = self.pulled;
        let node = self.cx.node().raw();
        // (the reason made only when traced: this is asked at every
        // command of a run that pulled a sibling)
        let refuse = |why: &dyn Fn() -> String| {
            if trace {
                eprintln!(
                    "giveback: refused (node {node}, pulled {pulled}): {}",
                    why()
                );
            }
            Ok(false)
        };
        if self.pulled == 0 {
            return Ok(false);
        }
        if self.pull_at.len() != self.pulled || self.pull_orig.len() != self.pulled {
            return refuse(&|| "an earlier sibling was not given back".to_owned());
        }
        // (one group of the run's own open is no bar: it is left to the
        // engine as the scope the given-back sibling runs in,
        // `Run::run_all`)
        if (self.open_span() && !self.can_leave_scope()) || self.aopen != 0 {
            return refuse(&|| {
                format!(
                    "open: {} nest levels, {} conditionals, {} alignments",
                    self.nest.len(),
                    self.conds.len(),
                    self.aopen
                )
            });
        }
        if self.claim_rest {
            return refuse(&|| "the rest claimed".to_owned());
        }
        let Some(at) = self.pull_at[self.pulled - 1] else {
            return refuse(&|| "the sibling's text joined a continuation line".to_owned());
        };
        if !self.giveback_only_allows() {
            return refuse(&|| "PHITEX_GIVEBACK_ONLY".to_owned());
        }
        if let Some(orig) = self.pull_orig.get(self.pulled - 1).cloned().flatten() {
            let given = self.give_back_resumed(at, orig)?;
            if trace {
                eprintln!(
                    "giveback: {} (after a resume) by node {}",
                    if given {
                        "given back"
                    } else {
                        "refused: the resumed sibling's check"
                    },
                    self.cx.node().raw()
                );
            }
            return Ok(given);
        }
        // (only the sibling's first segment reached)
        if self.input.text_at != at + 1 {
            return refuse(&|| {
                format!(
                    "the sibling's segments reached: {} (first at {at})",
                    self.input.text_at
                )
            });
        }
        // the levels left, spent ones aside: the sibling's
        let left: Vec<crate::input::Level> = self
            .input
            .levels
            .iter()
            .copied()
            .filter(|l| match l.kind {
                kind::TOKS => l.a < l.b,
                kind::LINE => self.input.line(*l).has_more(),
                _ => self.input.file(*l).has_lines(),
            })
            .collect();
        let first = self.input.text[at].clone();
        let ok = match (&first, left.as_slice()) {
            // a token list read not at all (the token stepped back)
            (Seg::Toks(t), [l]) => {
                l.kind == kind::TOKS && self.input.arena.get(l.rest()) == t.as_slice()
            }
            // a line of which one token was read and put back
            (Seg::Line(orig), [line, back]) => {
                if line.kind != kind::LINE
                    || back.kind != kind::TOKS
                    || back.ty != crate::input::ty::BACKED_UP
                    || back.b - back.a != 1
                {
                    return refuse(&|| {
                        format!(
                            "levels left: a line and a token list of kind {} type {} length {}",
                            back.kind,
                            back.ty,
                            back.b - back.a
                        )
                    });
                }
                let peeked = self.input.arena.at(back.a);
                let (loc, state, endch) = {
                    let x = self.input.line(*line);
                    (x.loc, x.state, x.endch)
                };
                self.same_first_token(orig.clone(), peeked, loc, state, endch)?
            }
            _ => {
                return refuse(&|| {
                    format!(
                        "{} levels left over a {} first segment",
                        left.len(),
                        if matches!(first, Seg::Toks(_)) {
                            "token"
                        } else {
                            "line"
                        }
                    )
                });
            }
        };
        if !ok {
            return refuse(&|| "the first token or its position differs re-read now".to_owned());
        }
        if trace {
            let sib = self
                .cx
                .following(self.pulled - 1)
                .and_then(|(_, a)| a.first().map(|&x| self.v.render(x)))
                .unwrap_or_default();
            eprintln!(
                "giveback: given back by node {}: {}",
                self.cx.node().raw(),
                sib.chars().take(100).collect::<String>()
            );
        }
        self.drop_given_back(at);
        Ok(true)
    }

    /// (debug, `PHITEX_GIVEBACK_ONLY=k`: only the k-th distinct sibling
    /// text met by a give-back, in the order first met, may be given
    /// back; the others are refused)
    fn giveback_only_allows(&mut self) -> bool {
        static K: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
        static SEEN: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        let Some(k) = *K.get_or_init(|| {
            std::env::var("PHITEX_GIVEBACK_ONLY")
                .ok()
                .and_then(|x| x.parse().ok())
        }) else {
            return true;
        };
        let sib = self
            .cx
            .following(self.pulled - 1)
            .and_then(|(_, a)| a.first().map(|&x| self.v.render(x)))
            .unwrap_or_default();
        let mut seen = SEEN.locked();
        let i = seen.iter().position(|x| *x == sib).unwrap_or_else(|| {
            seen.push(sib.clone());
            seen.len() - 1
        });
        if crate::exec::obs_flag!("PHITEX_GIVEBACK_TRACE") {
            eprintln!(
                "giveback-only: candidate {} (allowed {}): {}",
                i + 1,
                i + 1 == k,
                sib.chars().take(100).collect::<String>()
            );
        }
        i + 1 == k
    }

    /// [`Run::give_back_peeked`] after a park: the peeked token is the
    /// text segment at `at`, the rest of the sibling's first line the one
    /// after it, neither reached yet; `orig` is that line unread.
    fn give_back_resumed(&mut self, at: usize, orig: crate::input::Line) -> R<bool> {
        if self.input.text_at != at
            || self.input.levels.iter().any(|l| match l.kind {
                kind::TOKS => l.a < l.b,
                kind::LINE => self.input.line(*l).has_more(),
                _ => self.input.file(*l).has_lines(),
            })
        {
            return Ok(false);
        }
        let (Some(Seg::Toks(t)), Some(Seg::Line(rest))) =
            (self.input.text.get(at), self.input.text.get(at + 1))
        else {
            return Ok(false);
        };
        let ([tok], loc, state, endch) = (t.as_slice(), rest.loc, rest.state, rest.endch) else {
            return Ok(false);
        };
        let tok = *tok;
        if !self.same_first_token(orig, tok, loc, state, endch)? {
            return Ok(false);
        }
        self.drop_given_back(at);
        Ok(true)
    }

    /// Ends the run's hold on the last pulled sibling, whose text begins
    /// at `at`: its levels popped, its text dropped, unclaimed.
    fn drop_given_back(&mut self, at: usize) {
        // (its first line was counted as it began, §362: the sibling's
        // node counts it again)
        let begun = match (self.input.text.get(at), self.input.text.get(at + 1)) {
            (Some(Seg::Line(l)), _) if l.starts => Some(l),
            (Some(Seg::Toks(_)), Some(Seg::Line(l))) if l.starts && l.endch != -2 => Some(l),
            _ => None,
        };
        if let Some(l) = begun {
            self.muts += 1;
            self.lines_entered -= 1;
            self.lines_read -= 1;
            // (and in the lines the run contributes by file,
            // `Run::doc_line_begun`, as §362 counted it there)
            if l.lineno == 0 && !l.rescanned {
                let name = l.origin.map_or(phitex_defuse::ValueId::UNIT, |o| o.0);
                if let Some(k) = self.doc_lines.iter().position(|(n, _)| *n == name) {
                    self.doc_lines[k].1 -= 1;
                    if self.doc_lines[k].1 <= 0 {
                        self.doc_lines.remove(k);
                    }
                }
            }
        }
        while self.input.pop().is_some() {}
        self.input.text.truncate(at);
        self.input.text_at = at;
        self.muts += 1;
        self.pulled -= 1;
        self.pull_at.pop();
        self.pull_orig.pop();
        self.pull_box.pop();
        self.pull_lev = None;
    }

    /// Whether `line`, read from its start under the codes in force now,
    /// gives `tok` first and stands at `loc`/`state` after it.
    fn same_first_token(
        &mut self,
        line: crate::input::Line,
        tok: Tok,
        loc: usize,
        state: u8,
        endch: i64,
    ) -> R<bool> {
        let counted = (self.lines_entered, self.lines_read);
        let doc_lines = self.doc_lines.clone();
        let saved = std::mem::replace(&mut self.input, crate::input::Input::new());
        let (eof, no_pull) = (self.eof, self.no_pull);
        self.eof = true;
        self.no_pull = true;
        self.input.push_line(line);
        let got = self.next_tok();
        // (the same token, the line where it was left, and the same
        // `\endlinechar` after it, §360)
        let same = matches!(got, Ok(Some(t)) if t == tok)
            && self
                .input
                .top_line()
                .is_some_and(|x| x.loc == loc && x.state == state && x.endch == endch);
        (self.lines_entered, self.lines_read) = counted;
        self.doc_lines = doc_lines;
        self.input = saved;
        self.eof = eof;
        self.no_pull = no_pull;
        got.map(|_| same)
    }

    /// Whether the input is down to one line that a node boundary cut
    /// (it does not end its line and nothing of the text follows it), so
    /// the next sibling's first segment continues it.
    fn top_line_is_cut(&self) -> bool {
        self.input.depth() == 1
            && self.input.text_rest().is_empty()
            && self.input.top_line().is_some_and(|s| !s.ends)
    }

    /// Whether the first character of the next sibling's text, if it
    /// continues this line, is a letter (§356: a control word's name
    /// goes on).
    fn next_continues_name(&mut self) -> R<bool> {
        if !self.top_line_is_cut() {
            return Ok(false);
        }
        let Some((op, args)) = self.cx.following(self.pulled) else {
            return Ok(false);
        };
        if op != self.ops.exec {
            return Ok(false);
        }
        match crate::run::first_seg(self.v, &args) {
            Some(Seg::Line(s)) if !s.starts => match s.buf.first() {
                Some(&c) => Ok(matches!(self.cat(c)?, 7 | 11)),
                None => Ok(false),
            },
            _ => Ok(false),
        }
    }

    /// Whether the next sibling's text continues this line (the line on
    /// top is the last segment of the text, not ended).
    fn next_is_continuation(&mut self) -> bool {
        if !self.top_line_is_cut() {
            return false;
        }
        match self.cx.following(self.pulled) {
            Some((op, args)) if op == self.ops.exec => {
                matches!(crate::run::first_seg(self.v, &args), Some(Seg::Line(s)) if !s.starts)
            }
            _ => false,
        }
    }

    /// Whether there is input left without claiming more siblings.
    pub(crate) fn has_input(&mut self) -> bool {
        // (spent lists popped first, §325: the level on top answers)
        self.input.pop_spent();
        // (the levels on top: a list with tokens left, a line with
        // characters left, a file with lines; a document file's lines
        // left for the region children stop the run, also inside groups
        // of the run's own it leaves as scopes: what follows the file,
        // its end marker included, is read after them, [`Run::leftover`])
        for l in self.input.levels.iter().rev() {
            let more = match l.kind {
                kind::TOKS => l.a < l.b,
                kind::LINE => self.input.line(*l).has_more(),
                _ => {
                    let f = self.input.file(*l);
                    if !f.has_lines() {
                        continue;
                    }
                    if self.stops_at_doc_file() && self.doc_file(f) {
                        return false;
                    }
                    true
                }
            };
            if more {
                return true;
            }
        }
        for s in self.input.text_rest() {
            let more = match s {
                Seg::Toks(t) => !t.is_empty(),
                Seg::Line(l) => l.has_more(),
                Seg::File(f) => {
                    if !f.has_lines() {
                        continue;
                    }
                    if self.stops_at_doc_file() && self.doc_file(f) {
                        return false;
                    }
                    true
                }
            };
            if more {
                return true;
            }
        }
        false
    }

    /// Whether the input left is blank: no token list with tokens and no
    /// file with lines, the lines left only spacers (or what a comment
    /// character ends) and their ends a space or nothing, an empty line's
    /// `\par` only where `\par` is the primitive (§1070: in vertical mode
    /// a space does nothing, and `\par` adds nothing to the list).
    pub(crate) fn rest_blank(&mut self) -> R<bool> {
        self.input.pop_spent();
        let mut lines: Vec<(Vec<u32>, u8, Option<i64>)> = Vec::new();
        for l in self.input.levels.iter().rev() {
            match l.kind {
                kind::TOKS => {
                    if l.a < l.b {
                        return Ok(false);
                    }
                }
                kind::LINE => {
                    let ln = self.input.line(*l);
                    let end = (ln.ends && ln.endch < -1).then_some(ln.endch);
                    lines.push((ln.buf[ln.loc.min(ln.buf.len())..].to_vec(), ln.state, end));
                }
                _ => {
                    if self.input.file(*l).has_lines() {
                        return Ok(false);
                    }
                }
            }
        }
        for sg in self.input.text_rest() {
            match sg {
                Seg::Toks(t) => {
                    if !t.is_empty() {
                        return Ok(false);
                    }
                }
                Seg::Line(ln) => {
                    let end = (ln.ends && ln.endch < -1).then_some(ln.endch);
                    lines.push((ln.buf[ln.loc.min(ln.buf.len())..].to_vec(), ln.state, end));
                }
                Seg::File(f) => {
                    if f.has_lines() {
                        return Ok(false);
                    }
                }
            }
        }
        for (chars, state, end) in lines {
            let mut state = state;
            let mut cut = false;
            for c in chars {
                match self.cat(c)? {
                    9 => {}
                    10 => {
                        if state == MID_LINE {
                            state = SKIP_BLANKS;
                        }
                    }
                    14 => {
                        cut = true;
                        break;
                    }
                    _ => return Ok(false),
                }
            }
            let Some(_) = end.filter(|_| !cut) else { continue };
            let e = self.endlinechar()?;
            let Ok(e) = u32::try_from(e) else { continue };
            if e > 255 {
                continue;
            }
            match self.cat(e)? {
                9 | 10 => {}
                5 if state != NEW_LINE => {}
                5 => {
                    let m = self.mean_of(crate::expand::par_tok())?;
                    if m.cmd_chr().0 != cmd::PAR_END {
                        return Ok(false);
                    }
                }
                _ => return Ok(false),
            }
        }
        Ok(true)
    }

    /// Whether a document file reached here stops the run (its lines
    /// the region children, [`Run::leftover`]): a document's run with no
    /// level of its own open. Open conditionals do not keep it reading:
    /// they are all in a branch being taken (a skipped branch is passed
    /// within one command), and the stack goes on as a field
    /// ([`crate::run::COND_STACK`]).
    ///
    /// A file reached inside a group of the run's own is read here, the
    /// group's close with it (§282 by the run's own save stack). Left to
    /// the engine as a scope around the file's pieces, a paragraph the
    /// file leaves open made its last piece read on through the group's
    /// close and the text after it, its view still holding the group's
    /// local bindings: a no-op `\let` made in the group outlived it.
    pub(crate) fn stops_at_doc_file(&self) -> bool {
        self.doc && self.nest.is_empty()
    }

    /// Whether the next input left is a document file's lines (what
    /// [`Run::has_input`] stops at).
    pub(crate) fn next_is_doc_file(&self) -> bool {
        for l in self.input.levels.iter().rev() {
            match l.kind {
                kind::TOKS if l.a < l.b => return false,
                kind::LINE if self.input.line(*l).has_more() => return false,
                kind::TOKS | kind::LINE => {}
                _ => {
                    let f = self.input.file(*l);
                    if f.has_lines() {
                        return self.doc_file(f);
                    }
                }
            }
        }
        for s in self.input.text_rest() {
            match s {
                Seg::Toks(t) if !t.is_empty() => return false,
                Seg::Line(l) if l.has_more() => return false,
                Seg::File(f) if f.has_lines() => return self.doc_file(f),
                _ => {}
            }
        }
        false
    }

    /// Whether a file `\input` opened is a document's (its rest becomes
    /// paragraph regions), not a format's (run line by line).
    pub(crate) fn doc_file(&self, f: &crate::run::FileRest) -> bool {
        // (a file the client cut: a document's, `api::DocFile`)
        if f.cut.is_some() {
            return true;
        }
        // (a file the job writes: as its writer found it, `ops::Files`)
        if f.units.is_some() {
            return f.sliceable;
        }
        // (the same test as for a file the text names, `inputs.rs`: a
        // file that ends itself early or reads its own line numbers is
        // read inline)
        phitex_source::pass::FileKind::from_path(&crate::val::str_of(self.v, f.name))
            == phitex_source::pass::FileKind::Document
            && crate::hints::document_text_of(self.v, f.id)
    }

    // ---------------------------------------------------- line scanning

    /// `get_next` on the line on top (§343–§356); `None` when the line
    /// is finished.
    #[allow(clippy::too_many_lines)]
    fn src_next(&mut self) -> R<Option<Tok>> {
        // §362: a line is read when it is reached
        if let Some(s) = self.input.top_line()
            && s.endch == -2
        {
            let (starts, ends) = (s.starts, s.ends);
            // §360: `\endlinechar` as it is when the line is read goes
            // after its last character (an assignment on the line itself
            // reaches the next line only: `\endlinechar=-1` on a line
            // still ends that line)
            let e = if ends {
                i64::from(self.endlinechar()?)
            } else {
                -1
            };
            // §362: a line is counted as it begins
            if starts {
                self.muts += 1;
                self.lines_entered += 1;
                self.lines_read += 1;
                // (a `\read` line is no file's, §483: `line` stays)
                if let Some(s) = self.input.top_line()
                    && s.lineno == 0
                    && !s.rescanned
                {
                    let name = s.origin.map_or(phitex_defuse::ValueId::UNIT, |o| o.0);
                    self.doc_line_begun(name);
                }
            }
            if let Some(s) = self.input.top_line_mut() {
                if (0..256).contains(&e) {
                    s.buf.push(u32::try_from(e).unwrap_or(13));
                }
                s.endch = e;
                if starts {
                    s.state = NEW_LINE;
                }
            }
        }
        loop {
            let (c, state) = {
                let Some(s) = self.input.top_line_mut() else {
                    return Ok(None);
                };
                if s.loc >= s.buf.len() {
                    return Ok(None);
                }
                let c = s.buf[s.loc];
                s.loc += 1;
                (c, s.state)
            };
            // (a comment's mark of the canonical text read as text:
            // `api::canonical_text`)
            if c == 0 && self.splice_mark()? {
                continue;
            }
            let mut c = c;
            // reswitch
            loop {
                let cat = self.cat(c)?;
                match cat {
                    // §345
                    x if x == 9 || (x == 10 && state != MID_LINE) => break,
                    0 => return self.scan_cs().map(Some),
                    13 => {
                        self.set_state(MID_LINE);
                        return Ok(Some(Tok::ch(13, c)));
                    }
                    7 => {
                        // `^^` cut by a node boundary
                        let near_end = self
                            .input
                            .top_line()
                            .is_some_and(|s| s.loc + 2 > s.buf.len());
                        if near_end && self.next_is_continuation() {
                            self.pull()?;
                            self.join_continuation()?;
                        }
                        if let Some(x) = self.sup_mark(c) {
                            c = x;
                            continue;
                        }
                        self.set_state(MID_LINE);
                        return Ok(Some(Tok::ch(7, c)));
                    }
                    15 => break, // §346: an invalid character is skipped
                    10 => {
                        // §349
                        self.set_state(SKIP_BLANKS);
                        return Ok(Some(Tok::ch(10, 32)));
                    }
                    5 => {
                        self.finish_line();
                        return Ok(match state {
                            MID_LINE => Some(Tok::ch(10, 32)),   // §348
                            SKIP_BLANKS => break,                // §350
                            _ => Some(crate::expand::par_tok()), // §351
                        });
                    }
                    14 => {
                        self.finish_line();
                        break;
                    }
                    _ => {
                        self.set_state(MID_LINE);
                        return Ok(Some(Tok::ch(cat, c)));
                    }
                }
            }
            // goto switch
            let done = self.input.top_line().is_none_or(|s| s.loc >= s.buf.len());
            if done {
                return Ok(None);
            }
        }
    }

    /// `\endlinechar` (§360), cached while the table is unchanged (a
    /// definition of any slot invalidates it: one compare a line).
    #[inline]
    pub(crate) fn endlinechar(&mut self) -> R<i32> {
        let g = self.st.any_gen();
        if let Some((at, e)) = self.elc
            && at == g
        {
            return Ok(e);
        }
        let e = self.ipar(crate::fields::Par::endlinechar)?;
        self.elc = Some((self.st.any_gen(), e));
        Ok(e)
    }

    /// The line on top is the last segment of the text and a sibling's
    /// first segment continues it: the sibling's segment, just pulled to
    /// the back of the text, joins the line on top. When the join
    /// completes a line already reached, `\endlinechar` as it is now goes
    /// after it (§360); it is read before anything changes, so a halt
    /// there rewinds cleanly.
    fn join_continuation(&mut self) -> R<()> {
        let Some(&top) = self.input.top() else {
            return Ok(());
        };
        if top.kind != kind::LINE {
            return Ok(());
        }
        let at = self.input.text_at;
        if at >= self.input.text.len() {
            return Ok(());
        }
        let Seg::Line(next) = &self.input.text[at] else {
            return Ok(());
        };
        if next.starts {
            return Ok(());
        }
        let completes = next.ends && self.input.line(top).endch != -2;
        let e = if completes {
            i64::from(self.ipar(crate::fields::Par::endlinechar)?)
        } else {
            -1
        };
        let Seg::Line(next) = self.input.text.remove(at) else {
            return Ok(());
        };
        // (a sibling whose text began here now continues the line on top)
        for p in &mut self.pull_at {
            *p = match *p {
                Some(x) if x == at => None,
                Some(x) if x > at => Some(x - 1),
                q => q,
            };
        }
        let line = self.input.line_mut(top);
        line.buf.extend_from_slice(&next.buf);
        line.ends = next.ends;
        if completes {
            if (0..256).contains(&e) {
                line.buf.push(u32::try_from(e).unwrap_or(13));
            }
            line.endch = e;
        }
        Ok(())
    }

    #[inline]
    fn set_state(&mut self, st: u8) {
        if let Some(s) = self.input.top_line_mut() {
            s.state = st;
        }
    }

    /// The rest of the line goes, its end too (§347, §348).
    fn finish_line(&mut self) {
        if let Some(s) = self.input.top_line_mut() {
            s.loc = s.buf.len();
            if s.endch < -1 {
                // the end has not been reached: nothing will follow
                s.endch = -1;
            }
        }
    }

    /// §352: an expanded character `^^A` or `^^df` after the `sup_mark`
    /// `c` just read.
    fn sup_mark(&mut self, c: u32) -> Option<u32> {
        let s = self.input.top_line_mut()?;
        let limit = s.buf.len().checked_sub(1)?;
        let loc = s.loc;
        if loc < s.buf.len() && s.buf[loc] == c && loc < limit {
            let c1 = s.buf[loc + 1];
            if c1 < 0o200 {
                s.loc = loc + 2;
                if is_hex(c1) && s.loc <= limit {
                    let c2 = s.buf[s.loc];
                    if is_hex(c2) {
                        s.loc += 1;
                        return Some(hex(c1) * 16 + hex(c2));
                    }
                }
                return Some(if c1 < 0o100 { c1 + 0o100 } else { c1 - 0o100 });
            }
        }
        None
    }

    /// §354–§356: a control sequence after an escape character.
    fn scan_cs(&mut self) -> R<Tok> {
        loop {
            // a name cut by a node boundary goes on in the next node
            if self.input.depth() == 1
                && let Some(s) = self.input.top_line()
                && !s.ends
            {
                let rest: Vec<u32> = s.buf[s.loc.min(s.buf.len())..].to_vec();
                let mut letters = true;
                for &c in &rest {
                    if !matches!(self.cat(c)?, 7 | 11) {
                        letters = false;
                        break;
                    }
                }
                let last_sup = match rest.last() {
                    Some(&c) => self.cat(c)? == 7,
                    None => false,
                };
                let more = if rest.is_empty() || (letters && last_sup) {
                    // `\` or `^^` at the cut: what follows is the name
                    self.next_is_continuation()
                } else {
                    letters && self.next_continues_name()?
                };
                if more && self.pull()? {
                    self.join_continuation()?;
                    continue;
                }
            }
            let Some(s) = self.input.top_line() else {
                return Ok(null_cs());
            };
            let (loc, limit_plus) = (s.loc, s.buf.len());
            if loc >= limit_plus {
                return Ok(null_cs());
            }
            // start_cs
            let mut k = loc;
            let c0 = self.buf_at(k);
            let mut cat = self.cat(c0)?;
            k += 1;
            self.set_state(if cat == 11 || cat == 10 {
                SKIP_BLANKS
            } else {
                MID_LINE
            });
            if cat == 11 && k < limit_plus {
                // §356
                loop {
                    let c = self.buf_at(k);
                    cat = self.cat(c)?;
                    k += 1;
                    if cat != 11 || k >= limit_plus {
                        break;
                    }
                }
                if self.reduce_expanded(k, cat) {
                    continue;
                }
                if cat != 11 {
                    k -= 1;
                }
                if k > loc + 1 {
                    let t = self.cs_of_buf(loc, k);
                    if let Some(s) = self.input.top_line_mut() {
                        s.loc = k;
                    }
                    return Ok(t);
                }
            } else if self.reduce_expanded(k, cat) {
                continue;
            }
            // (a one-character name: through the run's cache, as a word)
            let t = self.cs_of_buf(loc, loc + 1);
            if let Some(s) = self.input.top_line_mut() {
                s.loc = loc + 1;
            }
            return Ok(t);
        }
    }

    /// The control sequence named by `buf[lo..hi]` of the line on top:
    /// through the run's own cache first (one hash of the characters, no
    /// string), then the process-wide table (§259 `id_lookup`).
    fn cs_of_buf(&mut self, lo: usize, hi: usize) -> Tok {
        let Some(s) = self.input.top_line() else {
            return null_cs();
        };
        let chars = &s.buf[lo..hi];
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &c in chars {
            h ^= u64::from(c);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        let slot = usize::try_from(h & (crate::run::CS_CACHE as u64 - 1)).unwrap_or(0);
        let (hh, t) = self.cs_cache[slot];
        if hh == h
            && let Some(n) = t.as_cs()
            && n.name()
                .chars()
                .eq(chars.iter().map(|&c| char::from_u32(c).unwrap_or('?')))
        {
            return t;
        }
        let mut name = std::mem::take(&mut self.name_buf);
        name.clear();
        name.extend(chars.iter().map(|&c| char::from_u32(c).unwrap_or('?')));
        let t = Tok::cs(&name);
        self.name_buf = name;
        self.cs_cache[slot] = (h, t);
        t
    }

    #[inline]
    fn buf_at(&self, k: usize) -> u32 {
        self.input
            .top_line()
            .and_then(|s| s.buf.get(k).copied())
            .unwrap_or(0)
    }

    /// §355: with `buffer[k-1]` of category `cat`, reduces an expanded code
    /// there; true if it did (`goto start_cs`).
    fn reduce_expanded(&mut self, k: usize, cat: u8) -> bool {
        let Some(s) = self.input.top_line_mut() else {
            return false;
        };
        let limit = s.buf.len() - 1;
        if k > limit || cat != 7 || k >= limit {
            return false;
        }
        let cur = s.buf[k - 1];
        if s.buf[k] != cur {
            return false;
        }
        let c = s.buf[k + 1];
        if c >= 0o200 {
            return false;
        }
        let mut d = 2;
        if is_hex(c) && k + 2 <= limit && is_hex(s.buf[k + 2]) {
            d = 3;
        }
        s.buf[k - 1] = if d > 2 {
            hex(c) * 16 + hex(s.buf[k + 2])
        } else if c < 0o100 {
            c + 0o100
        } else {
            c - 0o100
        };
        s.buf.drain(k..k + d);
        self.muts += 1;
        true
    }

    // --------------------------------------------------- end of input

    /// `\endinput`'s pruning (§362): the lines and files after the first
    /// line on the stack go (the rest of the current file), down to the
    /// file's end marker. True if the marker is on the stack.
    pub(crate) fn end_input_prune(&mut self) -> bool {
        let is_eof = |t: Tok| matches!(t.as_cs(), Some(n) if n.starts_with("\u{10FFFE}eof"));
        // the levels, top first
        let mut seen_line = false;
        let mut keep: Vec<Level> = Vec::with_capacity(self.input.levels.len());
        let mut found = false;
        for &l in self.input.levels.iter().rev() {
            if found {
                // (below the file's end marker: the files that \input
                // it go on, §362)
                keep.push(l);
                continue;
            }
            match l.kind {
                kind::LINE => {
                    if seen_line {
                        continue;
                    }
                    seen_line = true;
                    keep.push(l);
                }
                kind::FILE => {
                    if seen_line {
                        continue;
                    }
                    keep.push(l);
                }
                _ => {
                    if self.input.arena.get(l.rest()).iter().any(|&t| is_eof(t)) {
                        found = true;
                    }
                    keep.push(l);
                }
            }
        }
        keep.reverse();
        self.input.levels = keep;
        if found {
            return true;
        }
        // then the text not reached
        let at = self.input.text_at;
        let mut i = at;
        while i < self.input.text.len() {
            match &self.input.text[i] {
                Seg::Line(_) => {
                    if seen_line {
                        self.input.text.remove(i);
                        continue;
                    }
                    seen_line = true;
                }
                Seg::File(_) => {
                    if seen_line {
                        self.input.text.remove(i);
                        continue;
                    }
                }
                Seg::Toks(t) => {
                    if t.iter().any(|&t| is_eof(t)) {
                        return true;
                    }
                }
            }
            i += 1;
        }
        false
    }

    /// `\endinput` sets `force_eof`: the file's end marker becomes its
    /// `F` form (no `\everyeof` at the end).
    pub(crate) fn force_eof_marker(&mut self) {
        let is_plain = |t: Tok| matches!(t.as_cs(), Some(n) if n.starts_with("\u{10FFFE}eof") && !n.ends_with(['S', 'F']));
        for l in self.input.levels.iter().rev() {
            if l.kind != kind::TOKS {
                continue;
            }
            let r = l.rest();
            for i in r.start..r.end {
                let t = self.input.arena.at(i);
                if is_plain(t)
                    && let Some(n) = t.as_cs()
                {
                    let f = Tok::cs(&format!("{n}F"));
                    self.input.arena.set(i, f);
                    return;
                }
            }
        }
        let at = self.input.text_at.min(self.input.text.len());
        for s in &mut self.input.text[at..] {
            if let Seg::Toks(t) = s
                && let Some(k) = t.iter().position(|&x| is_plain(x))
                && let Some(n) = t[k].as_cs()
            {
                t[k] = Tok::cs(&format!("{n}F"));
                return;
            }
        }
    }

    // ------------------------------------------------- mark and rewind

    /// The run's mutation counter (spec-final §2.4): bumped by every
    /// change a rewind of the input cannot undo (the table's definitions,
    /// saves and restores; the run's contributions, children, levels,
    /// conditionals, files read, siblings claimed). Equal at a stop to
    /// its value as the command began: nothing changed but the input.
    #[inline]
    pub(crate) fn mutations(&self) -> u64 {
        self.muts.wrapping_add(self.st.muts())
    }

    /// A command begins: marks the input and the state it starts from.
    #[inline]
    pub(crate) fn begin_command(&mut self) {
        self.pdf_cmd = 0;
        let sig = self.mutations();
        let mut m = std::mem::take(&mut self.mark);
        self.input.snapshot(&mut m.snap);
        m.active = true;
        m.sig = sig;
        m.effect = Some(self.effect);
        m.astate = self.astate;
        m.aopen = self.aopen;
        m.radix = self.radix;
        m.cur_order = Some(self.cur_order);
        m.no_pull = self.no_pull;
        self.mark = m;
    }

    /// After a stop inside a command: puts the input back as the command
    /// found it, so a resumed run executes the command from its start.
    /// False (the run must not park) if the command changed anything
    /// else, or the input in a way not recorded.
    pub(crate) fn rewind(&mut self) -> bool {
        if !self.mark.active
            || self.mutations() != self.mark.sig
            || Some(self.effect) != self.mark.effect
        {
            return false;
        }
        let m = std::mem::take(&mut self.mark);
        if !self.input.restore(&m.snap) {
            return false;
        }
        self.astate = m.astate;
        self.aopen = m.aopen;
        self.radix = m.radix;
        self.no_pull = m.no_pull;
        if let Some(o) = m.cur_order {
            self.cur_order = o;
        }
        true
    }
}

/// The command in progress (`Run::begin_command`): the input levels as
/// it found them and a fingerprint of the rest of the run's state.
#[derive(Default)]
pub(crate) struct Mark {
    pub active: bool,
    pub snap: input::Snapshot,
    /// The run's mutation counter then ([`Run::mutations`]).
    pub sig: u64,
    pub effect: Option<phitex_defuse::Effect>,
    pub astate: Option<i64>,
    pub aopen: i64,
    pub radix: i32,
    pub cur_order: Option<phitex_typeset::glue::Order>,
    pub no_pull: bool,
}

/// `null_cs` (§258), named once a process.
fn null_cs() -> Tok {
    static C: std::sync::OnceLock<Tok> = std::sync::OnceLock::new();
    *C.get_or_init(|| Tok::cs(""))
}

#[inline]
fn is_hex(c: u32) -> bool {
    char::from_u32(c).is_some_and(|ch| ch.is_ascii_digit() || ('a'..='f').contains(&ch))
}

#[inline]
fn hex(c: u32) -> u32 {
    if c <= u32::from(b'9') {
        c - u32::from(b'0')
    } else {
        c - u32::from(b'a') + 10
    }
}
