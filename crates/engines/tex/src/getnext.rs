//! `get_next` (§332–§365) over the flat input stack ([`crate::input`]):
//! the token path of one run. The fast path is one bounds test on the
//! level on top; everything else is behind `#[inline(never)]`.
//!
//! What a command found as its input is marked in O(1) (the levels are
//! four words each, copied) so a stopped command can be rewound and the
//! run parked ([`Run::begin_command`], [`Run::rewind`]); no hashing
//! per command.

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
    /// input.
    // the per-token hot path: inlined on purpose
    #[allow(clippy::inline_always)]
    #[inline(always)]
    pub(crate) fn get_next(&mut self) -> R<bool> {
        let b = self.fetch();
        match self.halt.take() {
            Some(h) => Err(h),
            None => Ok(b),
        }
    }

    /// `get_next` without a `Result`: a halt is raised ([`Run::raise`]).
    #[allow(clippy::inline_always)]
    #[inline(always)]
    pub(crate) fn fetch(&mut self) -> bool {
        // (the common case: a token of the list on top that is not an
        // internal marker, and no alignment entry to watch for)
        if let Some(tok) = self.input.next_plain_of_top() {
            self.set_cur_flag(tok);
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
    /// document without `\end` is the client's to report).
    pub(crate) fn get_next_or_end(&mut self) -> R<()> {
        if !self.get_next()? {
            if self.eof {
                return Err(Halt::Fatal);
            }
            self.cur = Cur {
                tok: crate::expand::relax_tok(),
                cmd: cmd::RELAX,
                chr: 256,
            };
        }
        Ok(())
    }

    /// `get_token` (§365).
    #[inline]
    pub(crate) fn get_token(&mut self) -> R<()> {
        self.get_next_or_end()
    }

    /// `get_x_token` (§380).
    pub(crate) fn get_x_token(&mut self) -> R<()> {
        loop {
            self.get_next_or_end()?;
            if self.cur.cmd <= cmd::MAX_COMMAND {
                return Ok(());
            }
            // (a macro: its body goes on top here, no expansion level)
            if (cmd::CALL..cmd::END_TEMPLATE).contains(&self.cur.cmd) {
                self.macro_call()?;
                continue;
            }
            if self.cur.cmd >= cmd::END_TEMPLATE && self.cur.cmd != cmd::DONT_EXPAND {
                return Ok(());
            }
            self.expand()?;
        }
    }

    /// `x_token` (§381).
    pub(crate) fn x_token(&mut self) -> R<()> {
        while self.cur.cmd > cmd::MAX_COMMAND {
            self.expand()?;
            self.get_next_or_end()?;
        }
        Ok(())
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

    /// The arena range of a macro body (§390: read in place): copied in
    /// on its first call in this run, found by its shared list after.
    /// Short bodies are copied every time (cheaper than the lookup).
    pub(crate) fn body_range(&mut self, body: &std::rc::Rc<[Tok]>) -> Range {
        if body.len() <= 8 {
            return self.input.arena.alloc(body);
        }
        let key = std::rc::Rc::as_ptr(body).cast::<Tok>() as usize;
        if let Some((_, r)) = self.body_cache.get(&key) {
            return *r;
        }
        let r = self.input.arena.alloc(body);
        self.body_cache.insert(key, (body.clone(), r));
        r
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
                self.pulled += 1;
                let segs = crate::run::decode_segs(self.v, &args);
                self.input.append_text(segs);
                Ok(true)
            }
            _ => {
                if self.top {
                    self.eof = true;
                    Ok(false)
                } else {
                    Err(Halt::Starve)
                }
            }
        }
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
    fn next_is_continuation(&self) -> bool {
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
    pub(crate) fn has_input(&self) -> bool {
        // (the levels on top: a list with tokens left, a line with
        // characters left, a file with lines; a document file's lines
        // left for the region children stop the run: what follows the
        // file, its end marker included, is read after them,
        // [`Run::leftover`])
        for l in self.input.levels.iter().rev() {
            let more = match l.kind {
                kind::TOKS => l.a < l.b,
                kind::LINE => self.input.line(*l).has_more(),
                _ => {
                    let f = self.input.file(*l);
                    if !f.has_lines() {
                        continue;
                    }
                    if self.doc && !self.open_span() && self.doc_file(f) {
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
                    if self.doc && !self.open_span() && self.doc_file(f) {
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

    /// Whether a file `\input` opened is a document's (its rest becomes
    /// paragraph regions), not a format's (run line by line).
    pub(crate) fn doc_file(&self, f: &crate::run::FileRest) -> bool {
        crate::hints::file_kind_of(self.v, f.id, f.name) == phitex_source::pass::FileKind::Document
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
                i64::from(self.ipar(crate::fields::Par::endlinechar)?)
            } else {
                -1
            };
            // §362: a line is counted as it begins
            if starts {
                self.lines_entered += 1;
                self.lines_read += 1;
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
                return Ok(Tok::cs(""));
            };
            let (loc, limit_plus) = (s.loc, s.buf.len());
            if loc >= limit_plus {
                return Ok(Tok::cs("")); // null_cs
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
            let c = self.buf_at(loc);
            if let Some(s) = self.input.top_line_mut() {
                s.loc = loc + 1;
            }
            let mut b = [0; 4];
            return Ok(Tok::cs(
                char::from_u32(c).unwrap_or('?').encode_utf8(&mut b),
            ));
        }
    }

    /// The control sequence named by `buf[lo..hi]` of the line on top:
    /// through the run's own cache first (one hash of the characters, no
    /// string), then the process-wide table (§259 `id_lookup`).
    fn cs_of_buf(&mut self, lo: usize, hi: usize) -> Tok {
        let Some(s) = self.input.top_line() else {
            return Tok::cs("");
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
        self.irreversible += 1;
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
                    if !found && self.input.arena.get(l.rest()).iter().any(|&t| is_eof(t)) {
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

    /// What a stopped command must not have changed to be undone by
    /// rewinding its input: the clocks and the sizes, copied (no hash).
    #[inline]
    fn state_sig(&self) -> Sig {
        let (a, b, c, d) = self.st.sig();
        let top = self.nest.last();
        Sig {
            st: (a, b, c, d),
            contribs: self.contribs.len(),
            kids: self.kids.len(),
            nest: self.nest.len(),
            conds: self.conds.len(),
            if_codes: self.if_codes.len(),
            next_lid: self.next_lid,
            irreversible: self.irreversible,
            fread: self.fread.len(),
            pulled: self.pulled,
            eof: self.eof,
            last_par: self.last_par,
            claim_rest: self.claim_rest,
            pdf_occ: self.pdf_occ,
            lines_entered: self.lines_entered,
            top_list: top.map_or(0, |l| l.list.len()),
            top_last: top
                .and_then(|l| l.list.last().map(|x| x.raw()))
                .unwrap_or(0),
            top_ag: top.map_or(0, |l| l.ag.len()),
            top_extra: top.map_or(0, |l| l.extra.len()),
            top_prev_graf: top.map_or(0, |l| l.prev_graf),
            below_list: self
                .nest
                .len()
                .checked_sub(2)
                .and_then(|i| self.nest.get(i))
                .map_or(0, |l| l.list.len()),
        }
    }

    /// A command begins: marks the input and the state it starts from.
    #[inline]
    pub(crate) fn begin_command(&mut self) {
        let sig = self.state_sig();
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
            || self.pg.is_some()
            || self.state_sig() != self.mark.sig
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

/// The run's state as a command began ([`Run::state_sig`]).
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Sig {
    st: (usize, usize, usize, u64),
    contribs: usize,
    kids: usize,
    nest: usize,
    conds: usize,
    if_codes: usize,
    next_lid: i64,
    irreversible: u64,
    fread: usize,
    pulled: usize,
    eof: bool,
    last_par: (i64, i64),
    claim_rest: bool,
    pdf_occ: usize,
    lines_entered: i64,
    top_list: usize,
    top_last: u64,
    top_ag: usize,
    top_extra: usize,
    top_prev_graf: i64,
    below_list: usize,
}

/// The command in progress (`Run::begin_command`): the input levels as
/// it found them and a fingerprint of the rest of the run's state.
#[derive(Default)]
pub(crate) struct Mark {
    pub active: bool,
    pub snap: input::Snapshot,
    pub sig: Sig,
    pub effect: Option<phitex_defuse::Effect>,
    pub astate: Option<i64>,
    pub aopen: i64,
    pub radix: i32,
    pub cur_order: Option<phitex_typeset::glue::Order>,
    pub no_pull: bool,
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
