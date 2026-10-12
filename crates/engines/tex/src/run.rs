//! One run of one command node: its input (the node's own text, then the
//! siblings it claims), `get_next` (§341–§365) over it, and the
//! definitions, contributions and children it makes, buffered until the
//! command is done.
//!
//! Everything here is local to one run: a small sequential scanner over
//! the node's own text. What TeX keeps between commands is read through
//! fields (`Run::get`), the scope kind and contributions.

use std::collections::VecDeque;
use std::rc::Rc;

use phitex_defuse::{Ctx, Effect, Field, FxHashMap, Locality, OpId, Stop, Value, ValueId, Values};

use crate::val::{Mean, Tok, cmd, int_of, str_of};

/// Why a run cannot finish its command now.
pub(crate) enum Halt {
    /// A value is not known yet (the engine re-runs the node).
    Stop(Stop),
    /// The command needs more input than the node can claim: a child at
    /// the end of its parent's text (the claim at the call measures what
    /// a body's tail takes, so this is a bug when it happens).
    Starve,
    /// The input ended for good inside a command (§360: in batch mode,
    /// "*** (job aborted, no legal \end found)").
    Fatal,
}

impl From<Stop> for Halt {
    fn from(s: Stop) -> Halt {
        Halt::Stop(s)
    }
}

pub(crate) type R<T> = Result<T, Halt>;

/// The operations, as registered.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Ops {
    pub exec: OpId,
    pub put: OpId,
    pub outpage: OpId,
}

/// A level of the input stack.
#[derive(Clone, Debug)]
pub(crate) enum Frame {
    /// A token list and the position in it.
    Toks(Rc<[Tok]>, usize),
    /// A line of a file (§300): its characters (with `\endlinechar`
    /// appended once read), position and state.
    Src(Src),
    /// The lines of a file not reached yet: its text (a string value)
    /// and the byte offset of the next line. A line becomes a [`Src`]
    /// frame when it is reached, so the rest of a file is one frame, and
    /// one child when a node leaves it (never one per line).
    File(FileRest),
}

#[derive(Clone, Debug)]
pub(crate) struct FileRest {
    pub id: ValueId,
    pub text: std::sync::Arc<phitex_defuse::Value>,
    pub off: usize,
    /// The file's name, for positions (a string value).
    pub name: ValueId,
}

impl FileRest {
    pub(crate) fn new(v: &Values, id: ValueId, off: usize, name: ValueId) -> FileRest {
        FileRest {
            id,
            text: v.get(id),
            off,
            name,
        }
    }

    /// The position of the next line in the file (its characters are the
    /// file's bytes).
    fn byte_pos(&self) -> usize {
        self.str()[..self.off].chars().count()
    }

    fn str(&self) -> &str {
        match &*self.text {
            phitex_defuse::Value::Str(s) => s,
            _ => "",
        }
    }

    /// Whether lines are left.
    pub(crate) fn has_lines(&self) -> bool {
        self.off < self.str().len()
    }

    /// The next line, with its end of line; the offset moves past it.
    fn take_line(&mut self) -> String {
        let s = self.str();
        let rest = &s[self.off..];
        let n = rest.find('\n').map_or(rest.len(), |k| k + 1);
        let line = rest[..n].to_owned();
        self.off += n;
        line
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Src {
    pub buf: Vec<u32>,
    pub loc: usize,
    /// 0 `mid_line`, 1 `skip_blanks`, 2 `new_line` (§303).
    pub state: u8,
    /// -2 until the segment is reached (`\endlinechar` not yet applied).
    pub endch: i64,
    /// Whether the segment begins a line (it is counted), and whether it
    /// ends one (`\endlinechar` goes after it).
    pub starts: bool,
    pub ends: bool,
    /// Where `buf[0]` is in a file: the file's name and byte position
    /// (lines of `\input` files; for diagnostics only).
    pub origin: Option<(ValueId, usize)>,
}

pub(crate) const MID_LINE: u8 = 0;
pub(crate) const SKIP_BLANKS: u8 = 1;
pub(crate) const NEW_LINE: u8 = 2;

/// The current token (`cur_tok`, `cur_cmd`, `cur_chr`, `cur_cs`).
#[derive(Clone, Debug)]
pub(crate) struct Cur {
    pub tok: Tok,
    pub cmd: u8,
    pub chr: i32,
    pub mean: Mean,
}

impl Cur {
    pub(crate) fn is_cs(&self) -> bool {
        self.tok.is_cs_or_active()
    }
}

#[allow(clippy::struct_excessive_bools)]
pub(crate) struct Run<'c, 'a> {
    pub cx: &'c mut Ctx<'a>,
    pub v: &'a Values,
    pub ops: Ops,
    pub frames: VecDeque<Frame>,
    pub pulled: usize,
    /// Whether the node is the document's text (no parent): the end of
    /// its following siblings is the end of the file.
    pub top: bool,
    /// Whether `\end` takes every following sibling.
    pub claim_rest: bool,
    /// In a trial (`measure_tail`): the scope kind its own commands
    /// opened and closed so far, in place of the node's.
    #[allow(clippy::option_option)]
    pub trial_kind: Option<Option<ValueId>>,
    /// In a trial, the groups it opens get list ids of their own.
    pub trial_ids: Option<std::cell::Cell<i64>>,
    /// While looking for the command's first token: a node's command
    /// begins in its own text (the next sibling's begins in its own).
    pub no_pull: bool,
    own: FxHashMap<Field, Option<ValueId>>,
    pub defs: Vec<(Field, ValueId, Locality)>,
    pub contribs: Vec<(Field, ValueId)>,
    pub kids: Vec<(OpId, Vec<ValueId>)>,
    pub effect: Effect,
    pub cur: Cur,
    /// Conditionals begun in this run: their `if_limit` (§489).
    pub conds: Vec<u8>,
    mcache: FxHashMap<ValueId, Mean>,
    /// `align_state` as this run has it (§309), once read.
    pub astate: Option<i64>,
    /// Lines entered by this run (for `\inputlineno`).
    pub lines_entered: i64,
    /// Whether the input is exhausted for good (the file ended).
    pub eof: bool,
    /// `radix` (§438) and `cur_order` (§447).
    pub radix: i32,
    pub cur_order: phitex_typeset::glue::Order,
    /// The objects this run made at read time in relaxed PDF mode (their
    /// owner keys: the node and the occurrence).
    pub pdf_occ: usize,
}

impl<'c, 'a> Run<'c, 'a> {
    pub(crate) fn new(cx: &'c mut Ctx<'a>, ops: Ops) -> Self {
        let v = cx.values();
        let op = cx.op();
        let code = if op == ops.exec {
            1
        } else if op == ops.put {
            2
        } else if op == ops.outpage {
            3
        } else {
            0
        };
        crate::exec::note(code, cx.node().raw());
        Run {
            cx,
            v,
            ops,
            frames: VecDeque::new(),
            pulled: 0,
            top: false,
            claim_rest: false,
            trial_kind: None,
            trial_ids: None,
            no_pull: false,
            own: FxHashMap::default(),
            defs: Vec::new(),
            contribs: Vec::new(),
            kids: Vec::new(),
            effect: Effect::None,
            cur: Cur {
                tok: Tok::C(0),
                cmd: 0,
                chr: 0,
                mean: Mean::Undefined,
            },
            conds: Vec::new(),
            mcache: FxHashMap::default(),
            astate: None,
            lines_entered: 0,
            eof: false,
            radix: 0,
            cur_order: phitex_typeset::glue::Order::Normal,
            pdf_occ: 0,
        }
    }

    pub(crate) fn node_raw(&self) -> i64 {
        let id = i64::from_ne_bytes(self.cx.node().raw().to_ne_bytes());
        match &self.trial_ids {
            Some(c) => {
                c.set(c.get() + 1);
                id ^ c
                    .get()
                    .wrapping_mul(0x9E37_79B9_7F4A_7C15_u64.cast_signed())
            }
            None => id,
        }
    }

    // ------------------------------------------------------------ fields

    pub(crate) fn get_f(&mut self, f: Field) -> R<Option<ValueId>> {
        if let Some(&x) = self.own.get(&f) {
            return Ok(x);
        }
        Ok(self.cx.use_field(f)?)
    }

    /// The file `key` from the client (`file:<key>`): `None` if it is
    /// absent; until the client supplies it or says it is absent, the
    /// node waits ([`phitex_defuse::Ctx::use_env`]). A file the job
    /// writes and reads back is a loop-back field: the previous round's.
    pub(crate) fn file(&mut self, key: &str) -> R<Option<ValueId>> {
        let v = self.v;
        let f = v.field(&format!("file:{key}"));
        let x = if self.cx.is_loopback(f) {
            self.get_f(f)?
        } else {
            self.cx.use_env(f)?
        };
        Ok(x.filter(|&x| !crate::val::list_of(v, x).is_empty()))
    }

    pub(crate) fn get(&mut self, name: &str) -> R<Option<ValueId>> {
        let f = self.v.field(name);
        self.get_f(f)
    }

    pub(crate) fn int(&mut self, name: &str) -> R<i64> {
        Ok(self.get(name)?.map_or(0, |x| int_of(self.v, x)))
    }

    pub(crate) fn int32(&mut self, name: &str) -> R<i32> {
        Ok(crate::val::i32_of(self.v, self.get(name)?))
    }

    pub(crate) fn param(&mut self, name: &str) -> R<i32> {
        self.int32(&format!("param:{name}"))
    }

    pub(crate) fn glue_param(&mut self, name: &str) -> R<phitex_typeset::glue::Glue> {
        let x = self.get(&format!("param:{name}"))?;
        Ok(crate::val::glue_of(self.v, x))
    }

    pub(crate) fn cat(&mut self, c: u32) -> R<u8> {
        Ok(u8::try_from(self.int(&format!("cat:{c}"))?).unwrap_or(12))
    }

    /// Defines `name` (§277–§279: `eq_define` / `geq_define`).
    pub(crate) fn set(&mut self, name: &str, val: ValueId, global: bool) {
        let f = self.v.field(name);
        self.set_f(f, val, global);
    }

    pub(crate) fn set_f(&mut self, f: Field, val: ValueId, global: bool) {
        self.own.insert(f, Some(val));
        self.defs.retain(|d| d.0 != f);
        self.defs.push((
            f,
            val,
            if global {
                Locality::Global
            } else {
                Locality::Local
            },
        ));
    }

    pub(crate) fn contribute(&mut self, name: &str, val: ValueId) {
        let f = self.v.field(name);
        self.contribs.push((f, val));
    }

    pub(crate) fn kid(&mut self, op: OpId, args: Vec<ValueId>) {
        self.kids.push((op, args));
    }

    /// A log event (laid out by the client, [`crate::log`]).
    pub(crate) fn log(&mut self, kind: &str, text: &str) -> R<()> {
        if kind == "error" {
            self.diag(true, text);
        }
        let nl = self.int("param:newlinechar")?;
        let e = self
            .v
            .list(vec![self.v.str(kind), self.v.str(text), self.v.int(nl)]);
        let s = self.v.seq_unit(e);
        self.contribute("log", s);
        Ok(())
    }

    /// A diagnostic for [`crate::Build::diagnostics`]: where the input is
    /// (the current line of a file, or the node's own text), and the
    /// message. `error` false: a warning.
    pub(crate) fn diag(&mut self, error: bool, msg: &str) {
        let v = self.v;
        let src = self.frames.iter().find_map(|f| match f {
            Frame::Src(s) => Some(s.clone()),
            _ => None,
        });
        let (file, at, seg, loc) = match &src {
            Some(s) => {
                let mut buf = s.buf.clone();
                if s.endch >= 0 && buf.len() > s.loc && s.ends {
                    // the end of line character TeX appended
                    buf.pop();
                }
                let seg: String = buf
                    .iter()
                    .map(|&c| char::from_u32(c).unwrap_or('?'))
                    .collect();
                let loc = s.loc.min(buf.len());
                match s.origin {
                    Some((n, a)) => (n, i64::try_from(a).unwrap_or(0), seg, loc),
                    None => (ValueId::UNIT, -1, seg, loc),
                }
            }
            None => (ValueId::UNIT, -1, String::new(), 0),
        };
        let node = i64::from_ne_bytes(self.cx.node().raw().to_ne_bytes());
        let e = v.list(vec![
            v.int(i64::from(error)),
            v.str(msg),
            file,
            v.int(at),
            v.int(node),
            v.str(&seg),
            v.int(i64::try_from(loc).unwrap_or(0)),
        ]);
        let s = v.seq_unit(e);
        self.contribute("diags", s);
    }

    // ----------------------------------------------------------- meanings

    pub(crate) fn mean_of(&mut self, t: &Tok) -> R<Mean> {
        match t.field_name() {
            Some(name) => {
                let x = self.get(&name)?;
                Ok(self.decode_mean(x))
            }
            None => Ok(Mean::Char(t.cat().unwrap_or(0), t.chr())),
        }
    }

    pub(crate) fn decode_mean(&mut self, x: Option<ValueId>) -> Mean {
        let Some(x) = x else { return Mean::Undefined };
        if let Some(m) = self.mcache.get(&x) {
            return m.clone();
        }
        let m = Mean::decode(self.v, Some(x));
        self.mcache.insert(x, m.clone());
        m
    }

    pub(crate) fn set_cur(&mut self, tok: Tok) -> R<()> {
        let mean = self.mean_of(&tok)?;
        let (c, h) = match (&tok, &mean) {
            (Tok::C(_), _) if !tok.is_cs_or_active() => (
                tok.cat().unwrap_or(0),
                i32::try_from(tok.chr()).unwrap_or(0),
            ),
            _ => mean.cmd_chr(),
        };
        self.cur = Cur {
            tok,
            cmd: c,
            chr: h,
            mean,
        };
        Ok(())
    }

    // -------------------------------------------------------------- input

    /// `back_input` (§325): `t` is read next.
    pub(crate) fn back_input(&mut self, t: Tok) {
        // §325: a brace put back is counted again when read
        if let Some(s) = self.astate
            && matches!(t, Tok::C(_))
            && !t.is_cs_or_active()
        {
            match t.cat() {
                Some(1) => self.set_astate(s - 1),
                Some(2) => self.set_astate(s + 1),
                _ => {}
            }
        }
        if let Some(Frame::Toks(_, _)) = self.frames.front() {
            // a fresh one-token list, as tex.web makes one
        }
        self.frames.push_front(Frame::Toks(Rc::from(vec![t]), 0));
    }

    /// `begin_token_list` (§323): `toks` are read next.
    pub(crate) fn push_toks(&mut self, toks: Vec<Tok>) {
        if !toks.is_empty() {
            self.frames.push_front(Frame::Toks(Rc::from(toks), 0));
        }
    }

    /// Takes the next sibling's text into the input (§362: the next line,
    /// or the rest of this one).
    pub(crate) fn pull(&mut self) -> R<bool> {
        if self.eof || self.no_pull {
            return Ok(false);
        }
        match self.cx.following(self.pulled) {
            Some((op, args)) if op == self.ops.exec => {
                self.pulled += 1;
                let frames = decode_frames(self.v, &args);
                self.push_back_frames(frames);
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

    /// Appends frames; a segment continuing the line of the last one joins
    /// it (a control sequence's name may run across nodes).
    pub(crate) fn push_back_frames(&mut self, frames: Vec<Frame>) {
        for f in frames {
            if let (Some(Frame::Src(last)), Frame::Src(next)) = (self.frames.back_mut(), &f)
                && !last.ends
                && !next.starts
            {
                last.buf.extend_from_slice(&next.buf);
                last.ends = next.ends;
                continue;
            }
            self.frames.push_back(f);
        }
    }

    /// Whether the first character of the next sibling's text, if it
    /// continues this line, is a letter (§356: a control word's name
    /// goes on).
    fn next_continues_name(&mut self) -> R<bool> {
        let Some(Frame::Src(last)) = self.frames.back() else {
            return Ok(false);
        };
        if last.ends {
            return Ok(false);
        }
        let Some((op, args)) = self.cx.following(self.pulled) else {
            return Ok(false);
        };
        if op != self.ops.exec {
            return Ok(false);
        }
        match decode_frames(self.v, &args).first() {
            Some(Frame::Src(s)) if !s.starts => match s.buf.first() {
                Some(&c) => Ok(matches!(self.cat(c)?, 7 | 11)),
                None => Ok(false),
            },
            _ => Ok(false),
        }
    }

    /// Whether the next sibling's text continues this line.
    fn next_is_continuation(&self) -> bool {
        let Some(Frame::Src(last)) = self.frames.back() else {
            return false;
        };
        if last.ends || self.frames.len() != 1 {
            return false;
        }
        match self.cx.following(self.pulled) {
            Some((op, args)) if op == self.ops.exec => {
                matches!(decode_frames(self.v, &args).first(), Some(Frame::Src(s)) if !s.starts)
            }
            _ => false,
        }
    }

    /// Whether there is input left without claiming more siblings.
    pub(crate) fn has_input(&self) -> bool {
        self.frames.iter().any(|f| match f {
            Frame::Toks(t, p) => *p < t.len(),
            Frame::Src(s) => s.loc < s.buf.len() || (s.ends && s.endch < -1),
            Frame::File(f) => f.has_lines(),
        })
    }

    /// `get_next` (§341): the next token, its meaning not yet looked up.
    /// `None` at the end of all input.
    pub(crate) fn next_tok(&mut self) -> R<Option<Tok>> {
        loop {
            let Some(front) = self.frames.front_mut() else {
                if self.pull()? {
                    continue;
                }
                return Ok(None);
            };
            match front {
                Frame::Toks(t, p) => {
                    if *p < t.len() {
                        let tok = t[*p].clone();
                        *p += 1;
                        return Ok(Some(tok));
                    }
                    self.frames.pop_front();
                }
                Frame::Src(_) => {
                    if let Some(t) = self.src_next()? {
                        return Ok(Some(t));
                    }
                    self.frames.pop_front();
                }
                Frame::File(f) => {
                    if f.has_lines() {
                        let at = f.byte_pos();
                        let name = f.name;
                        let line = f.take_line();
                        let args = line_args(self.v, &line);
                        for mut fr in decode_frames(self.v, &args).into_iter().rev() {
                            if let Frame::Src(s) = &mut fr {
                                s.origin = Some((name, at));
                            }
                            self.frames.push_front(fr);
                        }
                    } else {
                        self.frames.pop_front();
                    }
                }
            }
        }
    }

    /// `get_next` on the current line (§343–§356); `None` when the line
    /// is finished.
    #[allow(clippy::too_many_lines)]
    fn src_next(&mut self) -> R<Option<Tok>> {
        // §362: a line is read when it is reached
        if let Some(Frame::Src(s)) = self.frames.front()
            && s.endch == -2
        {
            // §362: a line is counted as it begins
            let starts = s.starts;
            if starts {
                self.lines_entered += 1;
                let line = self.v.int(1);
                self.contribute("lines", line);
            }
            if let Some(Frame::Src(s)) = self.frames.front_mut() {
                s.endch = -3;
                if starts {
                    s.state = NEW_LINE;
                }
            }
        }
        self.end_line_char()?;
        loop {
            let (c, state) = {
                let Some(Frame::Src(s)) = self.frames.front_mut() else {
                    return Ok(None);
                };
                if s.loc >= s.buf.len() {
                    return Ok(None);
                }
                let c = s.buf[s.loc];
                s.loc += 1;
                (c, s.state)
            };
            // the line's end is reached after its last character (§360)
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
                        let near_end = matches!(self.frames.front(), Some(Frame::Src(s)) if s.loc + 2 > s.buf.len());
                        if near_end && self.next_is_continuation() {
                            self.pull()?;
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
                            MID_LINE => Some(Tok::ch(10, 32)), // §348
                            SKIP_BLANKS => break,              // §350
                            _ => Some(Tok::cs("par")),         // §351
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
            self.end_line_char()?;
            let done = matches!(self.frames.front(), Some(Frame::Src(s)) if s.loc >= s.buf.len());
            if done {
                return Ok(None);
            }
        }
    }

    /// At the end of a segment that ends its line: `\endlinechar` goes
    /// after it (§360; read here, where the line's end is reached).
    fn end_line_char(&mut self) -> R<()> {
        let due = matches!(self.frames.front(), Some(Frame::Src(s)) if s.ends && s.endch == -3 && s.loc >= s.buf.len());
        if due {
            let e = self.int("param:endlinechar")?;
            if let Some(Frame::Src(s)) = self.frames.front_mut() {
                if (0..256).contains(&e) {
                    s.buf.push(u32::try_from(e).unwrap_or(13));
                }
                s.endch = e;
            }
        }
        Ok(())
    }

    fn set_state(&mut self, st: u8) {
        if let Some(Frame::Src(s)) = self.frames.front_mut() {
            s.state = st;
        }
    }

    /// The rest of the line goes, its end too (§347, §348).
    fn finish_line(&mut self) {
        if let Some(Frame::Src(s)) = self.frames.front_mut() {
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
        let Some(Frame::Src(s)) = self.frames.front_mut() else {
            return None;
        };
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
            if self.frames.len() == 1
                && let Some(Frame::Src(s)) = self.frames.front()
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
                    continue;
                }
            }
            let (loc, limit_plus) = match self.frames.front() {
                Some(Frame::Src(s)) => (s.loc, s.buf.len()),
                _ => return Ok(Tok::cs("")),
            };
            if loc >= limit_plus {
                return Ok(Tok::cs("")); // null_cs
            }
            // start_cs
            let mut k = loc;
            let c0 = self.buf(k);
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
                    let c = self.buf(k);
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
                    let name: String = (loc..k)
                        .map(|i| char::from_u32(self.buf(i)).unwrap_or('?'))
                        .collect();
                    if let Some(Frame::Src(s)) = self.frames.front_mut() {
                        s.loc = k;
                    }
                    return Ok(Tok::S(Rc::from(name)));
                }
            } else if self.reduce_expanded(k, cat) {
                continue;
            }
            let c = self.buf(loc);
            if let Some(Frame::Src(s)) = self.frames.front_mut() {
                s.loc = loc + 1;
            }
            return Ok(Tok::S(Rc::from(
                char::from_u32(c).unwrap_or('?').to_string(),
            )));
        }
    }

    fn buf(&self, k: usize) -> u32 {
        match self.frames.front() {
            Some(Frame::Src(s)) => s.buf.get(k).copied().unwrap_or(0),
            _ => 0,
        }
    }

    /// §355: with `buffer[k-1]` of category `cat`, reduces an expanded code
    /// there; true if it did (`goto start_cs`).
    fn reduce_expanded(&mut self, k: usize, cat: u8) -> bool {
        let Some(Frame::Src(s)) = self.frames.front_mut() else {
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
        true
    }

    /// `get_next` with the meaning (§357–§358).
    pub(crate) fn get_next(&mut self) -> R<bool> {
        loop {
            let t = loop {
                let Some(t) = self.next_tok()? else {
                    return Ok(false);
                };
                if let Tok::S(n) = &t {
                    if n.starts_with("\u{10FFFE}eof") {
                        // §362: the end of an \input file (or a pseudo file)
                        crate::etex::end_of_file(self, &n.clone())?;
                        continue;
                    }
                    if &**n == "\u{10FFFE}uend" {
                        // §324: the end of a u template
                        self.align_u_end()?;
                        continue;
                    }
                }
                break t;
            };
            if let Tok::S(n) = &t
                && let Some(k) = n.strip_prefix("\u{10FFFE}endv:")
            {
                // §789: the end of a v template (`endv`, §380)
                let chr = k.parse().unwrap_or(0);
                self.cur = Cur {
                    tok: t.clone(),
                    cmd: cmd::IGNORE,
                    chr,
                    mean: Mean::Prim("\u{10FFFE}endv", cmd::IGNORE, chr),
                };
                return Ok(true);
            }
            if !self.get_next_rest(t)? {
                return Ok(false);
            }
            if self.align_token()? {
                continue;
            }
            return Ok(true);
        }
    }

    fn get_next_rest(&mut self, t: Tok) -> R<bool> {
        if let Tok::S(n) = &t
            && &**n == "\u{10FFFE}notexpanded"
        {
            // §358
            let Some(t2) = self.next_tok()? else {
                return Ok(false);
            };
            self.set_cur(t2)?;
            if self.cur.cmd > cmd::MAX_COMMAND {
                self.cur.cmd = cmd::RELAX;
                self.cur.chr = crate::val::NO_EXPAND_FLAG;
                self.cur.mean = Mean::NotExpanded;
            }
            return Ok(true);
        }
        self.set_cur(t)?;
        Ok(true)
    }

    /// `get_next`, at the end of all input a `\relax` (the end of a
    /// document without `\end` is the client's to report).
    pub(crate) fn get_next_or_end(&mut self) -> R<()> {
        if !self.get_next()? {
            if self.eof {
                return Err(Halt::Fatal);
            }
            self.cur = Cur {
                tok: Tok::cs("\u{10FFFE}relax"),
                cmd: cmd::RELAX,
                chr: 256,
                mean: Mean::Prim("relax", cmd::RELAX, 256),
            };
        }
        Ok(())
    }

    /// `get_token` (§365).
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

    // ------------------------------------------------------------ the end

    /// What is left of the node's own and claimed text, and the tokens a
    /// command put back or an expansion produced, as child nodes: one per
    /// token (a run of characters is one node), and the text not yet read
    /// as one node per segment.
    pub(crate) fn leftover(&self) -> Vec<Vec<ValueId>> {
        let v = self.v;
        let mut out: Vec<Vec<ValueId>> = Vec::new();
        let mut run: Vec<ValueId> = Vec::new();
        let node = |toks: Vec<ValueId>| -> Vec<ValueId> {
            let l = v.list(toks);
            vec![v.list(vec![v.list(vec![v.str("t"), l])]), v.int(0)]
        };
        for f in &self.frames {
            match f {
                Frame::Toks(t, p) => {
                    for tok in &t[*p..] {
                        let ch = matches!(tok.cat(), Some(11 | 12));
                        if ch {
                            run.push(tok.encode(v));
                            continue;
                        }
                        if !run.is_empty() {
                            out.push(node(std::mem::take(&mut run)));
                        }
                        out.push(node(vec![tok.encode(v)]));
                    }
                }
                Frame::Src(s) => {
                    // a run of characters stays with the text after it,
                    // so ligatures and kerns join them (§1034)
                    let head = std::mem::take(&mut run);
                    let end_due = s.ends && s.endch < -1;
                    if s.loc >= s.buf.len() && !end_due {
                        if !head.is_empty() {
                            out.push(node(head));
                        }
                        continue;
                    }
                    let text: String = s.buf[s.loc.min(s.buf.len())..]
                        .iter()
                        .map(|&c| char::from_u32(c).unwrap_or('?'))
                        .collect();
                    let starts = s.starts && s.endch == -2;
                    let mut xs = vec![
                        v.str("s"),
                        v.str(&text),
                        v.int(i64::from(s.state)),
                        v.int(-2),
                        v.int(i64::from(starts) | (i64::from(end_due) << 1)),
                    ];
                    if let Some((name, at)) = s.origin {
                        let at = at + s.loc.min(s.buf.len());
                        xs.push(name);
                        xs.push(v.int(i64::try_from(at).unwrap_or(0)));
                    }
                    let fr = v.list(xs);
                    let mut frs = Vec::new();
                    if !head.is_empty() {
                        frs.push(v.list(vec![v.str("t"), v.list(head)]));
                    }
                    frs.push(fr);
                    out.push(vec![v.list(frs), v.int(0)]);
                }
                Frame::File(f) => {
                    if !run.is_empty() {
                        out.push(node(std::mem::take(&mut run)));
                    }
                    if f.has_lines() {
                        let fr = v.list(vec![
                            v.str("f"),
                            f.id,
                            v.int(i64::try_from(f.off).unwrap_or(0)),
                            f.name,
                        ]);
                        out.push(vec![v.list(vec![fr]), v.int(0)]);
                    }
                }
            }
        }
        if !run.is_empty() {
            out.push(node(run));
        }
        out
    }

    /// The tokens of an expansion may end in a command that reads on into
    /// the text after them (`\newcount\foo`: `\alloc@`'s fifth argument):
    /// their extent is decided here, at the call, by reading them in a
    /// trial run whose effects are dropped; the siblings the trial reads
    /// are claimed, so the expansion's children find them in their own
    /// text (a child never reads past its parent).
    pub(crate) fn measure_tail(&mut self) -> R<()> {
        if !self
            .frames
            .iter()
            .any(|f| matches!(f, Frame::Toks(t, p) if *p < t.len()))
        {
            return Ok(());
        }
        let snap = self.snapshot();
        let before = self.pulled;
        let mut steps = 0u32;
        self.trial_ids = Some(std::cell::Cell::new(0));
        let res: R<()> = (|| {
            // the tokens' extent: the trial ends when they are read (the
            // text after them is the siblings' own); `\end` ends it too,
            // since its repetitions wait on the page builder
            let toks_left = |r: &Self| {
                r.frames
                    .iter()
                    .any(|f| matches!(f, Frame::Toks(t, p) if *p < t.len()))
            };
            while toks_left(self) && steps < 100_000 {
                steps += 1;
                self.effect = phitex_defuse::Effect::None;
                self.command()?;
                if steps < 30 && std::env::var_os("PHITEX_TEX_TRACE").is_some() {
                    let md = self.mode()?;
                    eprintln!(
                        "    trial {:?} mode {} group {} lid {}",
                        self.cur.tok, md.m, md.group, md.lid
                    );
                }
                self.trial_scope()?;
                if self.cur.cmd == crate::val::cmd::STOP {
                    break;
                }
            }
            Ok(())
        })();
        self.trial_kind = None;
        self.trial_ids = None;
        let after = self.pulled;
        if std::env::var_os("PHITEX_TEX_TRACE").is_some() {
            let r = match &res {
                Ok(()) => "ok".to_owned(),
                Err(Halt::Stop(_)) => "stop".to_owned(),
                Err(Halt::Starve) => "starve".to_owned(),
                Err(Halt::Fatal) => "fatal".to_owned(),
            };
            eprintln!(
                "  tail: {steps} steps, pulled {}, {r}, top {}",
                after - before,
                self.top
            );
        }
        self.restore(snap);
        if let Err(Halt::Stop(s)) = res {
            return Err(Halt::Stop(s));
        }
        for _ in before..after {
            if !self.pull()? {
                break;
            }
        }
        if after > before {
            let n = self.v.int(i64::try_from(after - before).unwrap_or(0));
            self.contribute("tails", self.v.seq_unit(n));
        }
        Ok(())
    }

    /// The scope kind the node is in: in a trial, the one its own
    /// commands made.
    pub(crate) fn scope_kind(&mut self) -> R<Option<ValueId>> {
        match self.trial_kind {
            Some(k) => Ok(k),
            None => Ok(self.cx.scope_kind()?),
        }
    }

    /// A trial follows the scopes its commands open and close, so the
    /// commands after them see the mode and group they would (a kind
    /// keeps its parent at index 5).
    fn trial_scope(&mut self) -> R<()> {
        let v = self.v;
        let parent = |k: Option<ValueId>| {
            k.and_then(|k| crate::val::list_of(v, k).get(5).copied())
                .filter(|&p| p != ValueId::UNIT)
        };
        let mut cur = self.scope_kind()?;
        match self.effect {
            phitex_defuse::Effect::Close => cur = parent(cur),
            phitex_defuse::Effect::Scope(b) => {
                for _ in 0..b.closes {
                    cur = parent(cur);
                }
                if b.opens > 0 {
                    cur = b.kind;
                }
            }
            _ => return Ok(()),
        }
        self.trial_kind = Some(cur);
        Ok(())
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            frames: self.frames.clone(),
            pulled: self.pulled,
            own: self.own.clone(),
            defs: self.defs.clone(),
            contribs: self.contribs.clone(),
            kids: self.kids.clone(),
            effect: self.effect,
            cur: self.cur.clone(),
            conds: self.conds.clone(),
            lines_entered: self.lines_entered,
            eof: self.eof,
            claim_rest: self.claim_rest,
            astate: self.astate,
        }
    }

    fn restore(&mut self, s: Snapshot) {
        self.frames = s.frames;
        self.pulled = s.pulled;
        self.own = s.own;
        self.defs = s.defs;
        self.contribs = s.contribs;
        self.kids = s.kids;
        self.effect = s.effect;
        self.cur = s.cur;
        self.conds = s.conds;
        self.lines_entered = s.lines_entered;
        self.eof = s.eof;
        self.claim_rest = s.claim_rest;
        self.astate = s.astate;
    }

    /// Ends the run: definitions, contributions, children (what the
    /// command made, then the input left), the claim and the scope effect.
    pub(crate) fn finish(mut self) -> ValueId {
        for (f, x, l) in std::mem::take(&mut self.defs) {
            self.cx.define_with(f, x, l);
        }
        if !self.own.is_empty() {
            let names: Vec<ValueId> = self.own.keys().map(|f| f.0).collect();
            let l = self.v.list(names);
            let s = self.v.seq_unit(l);
            self.contribs.push((self.v.field("defs"), s));
        }
        for (f, x) in std::mem::take(&mut self.contribs) {
            self.cx.contribute(f, x);
        }
        for (op, args) in std::mem::take(&mut self.kids) {
            self.cx.child(op, args);
        }
        for args in self.leftover() {
            self.cx.child(self.ops.exec, args);
        }
        let mut n = self.pulled;
        if self.claim_rest {
            while matches!(self.cx.following(n), Some((op, _)) if op == self.ops.exec) {
                n += 1;
            }
        }
        self.cx.claim(n);
        self.cx.set_effect(self.effect);
        ValueId::UNIT
    }

    /// Ends a run that cannot finish: nothing defined, and the need, for
    /// the client (a file it has not supplied yet; or a child that ran out
    /// of text, which is a bug).
    pub(crate) fn stall(self, need: &str, what: ValueId) -> ValueId {
        let v = self.v;
        let id = v.int(i64::from_ne_bytes(self.cx.node().raw().to_ne_bytes()));
        let e = v.list(vec![v.str(need), id, what]);
        self.cx.contribute(v.field("needs"), v.seq_unit(e));
        self.cx.claim(0);
        self.cx.set_effect(Effect::None);
        ValueId::UNIT
    }
}

/// A run's state, to undo a trial.
struct Snapshot {
    frames: VecDeque<Frame>,
    pulled: usize,
    own: FxHashMap<Field, Option<ValueId>>,
    defs: Vec<(Field, ValueId, Locality)>,
    contribs: Vec<(Field, ValueId)>,
    kids: Vec<(OpId, Vec<ValueId>)>,
    effect: Effect,
    cur: Cur,
    conds: Vec<u8>,
    lines_entered: i64,
    eof: bool,
    claim_rest: bool,
    astate: Option<i64>,
}

fn is_hex(c: u32) -> bool {
    (u32::from(b'0')..=u32::from(b'9')).contains(&c)
        || (u32::from(b'a')..=u32::from(b'f')).contains(&c)
}

fn hex(c: u32) -> u32 {
    if c <= u32::from(b'9') {
        c - u32::from(b'0')
    } else {
        c - u32::from(b'a') + 10
    }
}

/// The frames of an `Exec` node's arguments.
pub(crate) fn decode_frames(v: &Values, args: &[ValueId]) -> Vec<Frame> {
    let mut out = Vec::new();
    let Some(&l) = args.first() else { return out };
    let Value::List(fs) = &*v.get(l) else {
        return out;
    };
    for &f in fs {
        let Value::List(xs) = &*v.get(f) else {
            continue;
        };
        match xs.first().map(|&t| str_of(v, t)).as_deref() {
            Some("t") => {
                let toks = crate::val::toks_of(v, xs.get(1).copied());
                out.push(Frame::Toks(Rc::from(toks), 0));
            }
            Some("f") => {
                if let (Some(&id), Some(&o)) = (xs.get(1), xs.get(2)) {
                    let off = usize::try_from(int_of(v, o)).unwrap_or(0);
                    let name = xs.get(3).copied().unwrap_or(ValueId::UNIT);
                    out.push(Frame::File(FileRest::new(v, id, off, name)));
                }
            }
            Some("s") => {
                let text = xs.get(1).map(|&t| str_of(v, t)).unwrap_or_default();
                out.push(Frame::Src(Src {
                    buf: text.chars().map(u32::from).collect(),
                    loc: 0,
                    state: u8::try_from(xs.get(2).map_or(2, |&x| int_of(v, x))).unwrap_or(2),
                    endch: xs.get(3).map_or(-2, |&x| int_of(v, x)),
                    starts: xs.get(4).is_none_or(|&x| int_of(v, x) & 1 != 0),
                    ends: xs.get(4).is_none_or(|&x| int_of(v, x) & 2 != 0),
                    origin: match (xs.get(5), xs.get(6)) {
                        (Some(&n), Some(&o)) => {
                            Some((n, usize::try_from(int_of(v, o)).unwrap_or(0)))
                        }
                        _ => None,
                    },
                }));
            }
            _ => {}
        }
    }
    out
}

/// A line already read (`\read`, §483): `\endlinechar` applied.
pub(crate) fn src_frame(line: &str, endlinechar: i64) -> Frame {
    let bytes = line.as_bytes();
    let mut end = bytes.len();
    while end > 0 && matches!(bytes[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    while end > 0 && matches!(bytes[end - 1], b' ' | b'\t') {
        end -= 1;
    }
    let mut buf: Vec<u32> = line[..end].chars().map(u32::from).collect();
    if (0..256).contains(&endlinechar) {
        buf.push(u32::try_from(endlinechar).unwrap_or(13));
    }
    Frame::Src(Src {
        buf,
        loc: 0,
        state: NEW_LINE,
        endch: endlinechar,
        starts: true,
        ends: true,
        origin: None,
    })
}

/// A source line as a top-level node's arguments: its characters with
/// trailing spaces removed (web2c's `input_line`), not yet read.
pub(crate) fn line_args(v: &Values, line: &str) -> Vec<ValueId> {
    let bytes = line.as_bytes();
    let mut end = bytes.len();
    while end > 0 && matches!(bytes[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    while end > 0 && matches!(bytes[end - 1], b' ' | b'\t') {
        end -= 1;
    }
    let text: String = bytes[..end].iter().map(|&b| char::from(b)).collect();
    let frame = v.list(vec![
        v.str("s"),
        v.str(&text),
        v.int(i64::from(NEW_LINE)),
        v.int(-2),
        v.int(3),
    ]);
    vec![v.list(vec![frame]), v.int(1)]
}

/// The text of a node from the document: a slice of its lines, as
/// segments (each the part of one line the slice holds).
pub(crate) fn slice_args(v: &Values, text: &str, starts_line: bool) -> Vec<ValueId> {
    let mut frames = Vec::new();
    let mut starts = starts_line;
    let mut rest = text;
    while !rest.is_empty() {
        let (seg, ends, next) = match rest.find('\n') {
            Some(k) => (&rest[..k], true, &rest[k + 1..]),
            None => (rest, false, ""),
        };
        let seg: String = if ends {
            seg.trim_end_matches([' ', '\t', '\r']).to_owned()
        } else {
            seg.to_owned()
        };
        let state = if starts { NEW_LINE } else { MID_LINE };
        let flags = i64::from(starts) | (i64::from(ends) << 1);
        frames.push(v.list(vec![
            v.str("s"),
            v.str(&seg),
            v.int(i64::from(state)),
            v.int(-2),
            v.int(flags),
        ]));
        starts = ends;
        rest = next;
    }
    vec![v.list(frames), v.int(1)]
}
