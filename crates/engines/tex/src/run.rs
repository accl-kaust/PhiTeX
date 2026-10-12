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
    /// The command needs more input than the node can claim.
    // REDUCE-GAP: a child cannot claim its ancestors' following siblings;
    // the client supplies them through `hint:<node>` fields.
    Starve,
    /// The command reads a file the client has not supplied yet.
    File(String),
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
    pub fin: OpId,
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
}

#[derive(Clone, Debug)]
pub(crate) struct Src {
    pub buf: Vec<u32>,
    pub loc: usize,
    /// 0 `mid_line`, 1 `skip_blanks`, 2 `new_line` (§303).
    pub state: u8,
    /// -2 until the line is read (`\endlinechar` not yet applied).
    pub endch: i64,
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

pub(crate) struct Run<'c, 'a> {
    pub cx: &'c mut Ctx<'a>,
    pub v: &'a Values,
    pub ops: Ops,
    pub frames: VecDeque<Frame>,
    pub pulled: usize,
    pub hint: i64,
    own: FxHashMap<Field, Option<ValueId>>,
    pub defs: Vec<(Field, ValueId, Locality)>,
    pub contribs: Vec<(Field, ValueId)>,
    pub kids: Vec<(OpId, Vec<ValueId>)>,
    pub effect: Effect,
    pub cur: Cur,
    /// Conditionals begun in this run: their `if_limit` (§489).
    pub conds: Vec<u8>,
    mcache: FxHashMap<ValueId, Mean>,
    /// Lines entered by this run (for `\inputlineno`).
    pub lines_entered: i64,
    /// Whether the run ends with input exhausted for good (`hint` -1).
    pub eof: bool,
    /// `radix` (§438) and `cur_order` (§447).
    pub radix: i32,
    pub cur_order: phitex_typeset::glue::Order,
}

impl<'c, 'a> Run<'c, 'a> {
    pub(crate) fn new(cx: &'c mut Ctx<'a>, ops: Ops) -> Self {
        let v = cx.values();
        Run {
            cx,
            v,
            ops,
            frames: VecDeque::new(),
            pulled: 0,
            hint: 0,
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
            lines_entered: 0,
            eof: false,
            radix: 0,
            cur_order: phitex_typeset::glue::Order::Normal,
        }
    }

    pub(crate) fn node_raw(&self) -> i64 {
        i64::from_ne_bytes(self.cx.node().raw().to_ne_bytes())
    }

    // ------------------------------------------------------------ fields

    pub(crate) fn get_f(&mut self, f: Field) -> R<Option<ValueId>> {
        if let Some(&x) = self.own.get(&f) {
            return Ok(x);
        }
        Ok(self.cx.use_field(f)?)
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
        self.defs
            .push((f, val, if global { Locality::Global } else { Locality::Local }));
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
        let nl = self.int("param:newlinechar")?;
        let e = self
            .v
            .list(vec![self.v.str(kind), self.v.str(text), self.v.int(nl)]);
        let s = self.v.seq_unit(e);
        self.contribute("log", s);
        Ok(())
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
            (Tok::C(_), _) if !tok.is_cs_or_active() => (tok.cat().unwrap_or(0), i32::try_from(tok.chr()).unwrap_or(0)),
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

    /// Takes the next sibling's text into the input.
    fn pull(&mut self) -> R<bool> {
        if self.eof {
            return Ok(false);
        }
        match self.cx.following(self.pulled) {
            Some((op, args)) if op == self.ops.exec => {
                self.pulled += 1;
                let frames = decode_frames(self.v, &args);
                self.frames.extend(frames);
                Ok(true)
            }
            _ => {
                if self.hint < 0 {
                    self.eof = true;
                    Ok(false)
                } else {
                    Err(Halt::Starve)
                }
            }
        }
    }

    /// Whether there is input left without claiming more siblings.
    pub(crate) fn has_input(&self) -> bool {
        self.frames.iter().any(|f| match f {
            Frame::Toks(t, p) => *p < t.len(),
            Frame::Src(s) => s.loc < s.buf.len() || s.endch == -2,
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
            let e = self.int("param:endlinechar")?;
            self.lines_entered += 1;
            let line = self.v.int(1);
            self.contribute("lines", line);
            if let Some(Frame::Src(s)) = self.frames.front_mut() {
                if (0..256).contains(&e) {
                    s.buf.push(u32::try_from(e).unwrap_or(13));
                }
                s.endch = e;
                s.state = NEW_LINE;
            }
        }
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
            let mut c = c;
            // reswitch
            loop {
                let cat = self.cat(c)?;
                match cat {
                    // §345
                    9 => break,
                    10 if state != MID_LINE => break,
                    0 => return self.scan_cs().map(Some),
                    13 => {
                        self.set_state(MID_LINE);
                        return Ok(Some(Tok::ch(13, c)));
                    }
                    7 => {
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
                            _ => Some(Tok::cs("par")),          // §351
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
            let done = matches!(self.frames.front(), Some(Frame::Src(s)) if s.loc >= s.buf.len());
            if done {
                return Ok(None);
            }
        }
    }

    fn set_state(&mut self, st: u8) {
        if let Some(Frame::Src(s)) = self.frames.front_mut() {
            s.state = st;
        }
    }

    fn finish_line(&mut self) {
        if let Some(Frame::Src(s)) = self.frames.front_mut() {
            s.loc = s.buf.len();
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
            self.set_state(if cat == 11 || cat == 10 { SKIP_BLANKS } else { MID_LINE });
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
                if self.reduce_expanded(k, cat)? {
                    continue;
                }
                if cat != 11 {
                    k -= 1;
                }
                if k > loc + 1 {
                    let name: String = (loc..k).map(|i| char::from_u32(self.buf(i)).unwrap_or('?')).collect();
                    if let Some(Frame::Src(s)) = self.frames.front_mut() {
                        s.loc = k;
                    }
                    return Ok(Tok::S(Rc::from(name)));
                }
            } else if self.reduce_expanded(k, cat)? {
                continue;
            }
            let c = self.buf(loc);
            if let Some(Frame::Src(s)) = self.frames.front_mut() {
                s.loc = loc + 1;
            }
            return Ok(Tok::S(Rc::from(char::from_u32(c).unwrap_or('?').to_string())));
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
    fn reduce_expanded(&mut self, k: usize, cat: u8) -> R<bool> {
        let Some(Frame::Src(s)) = self.frames.front_mut() else {
            return Ok(false);
        };
        let limit = s.buf.len() - 1;
        if k > limit || cat != 7 || k >= limit {
            return Ok(false);
        }
        let cur = s.buf[k - 1];
        if s.buf[k] != cur {
            return Ok(false);
        }
        let c = s.buf[k + 1];
        if c >= 0o200 {
            return Ok(false);
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
        Ok(true)
    }

    /// `get_next` with the meaning (§357–§358).
    pub(crate) fn get_next(&mut self) -> R<bool> {
        let t = loop {
            let Some(t) = self.next_tok()? else {
                return Ok(false);
            };
            if let Tok::S(n) = &t
                && &**n == "\u{10FFFE}eof"
            {
                // §362: the end of an \input file
                self.log("close", ")")?;
                continue;
            }
            break t;
        };
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

    /// The input left, as the frames of one child.
    pub(crate) fn leftover(&mut self) -> Option<Vec<ValueId>> {
        let v = self.v;
        let mut frames: Vec<ValueId> = Vec::new();
        let mut toks: Vec<ValueId> = Vec::new();
        let flush = |toks: &mut Vec<ValueId>, frames: &mut Vec<ValueId>| {
            if !toks.is_empty() {
                let l = v.list(std::mem::take(toks));
                frames.push(v.list(vec![v.str("t"), l]));
            }
        };
        for f in &self.frames {
            match f {
                Frame::Toks(t, p) => toks.extend(t[*p..].iter().map(|x| x.encode(v))),
                Frame::Src(s) => {
                    flush(&mut toks, &mut frames);
                    if s.loc >= s.buf.len() && s.endch != -2 {
                        continue;
                    }
                    let text: String = s.buf[s.loc..].iter().map(|&c| char::from_u32(c).unwrap_or('?')).collect();
                    frames.push(v.list(vec![
                        v.str("s"),
                        v.str(&text),
                        v.int(i64::from(s.state)),
                        v.int(s.endch),
                    ]));
                }
            }
        }
        flush(&mut toks, &mut frames);
        (!frames.is_empty()).then(|| vec![v.list(frames)])
    }

    /// Ends the run: definitions, contributions, children, the input left
    /// (one child), the claim and the scope effect.
    pub(crate) fn finish(mut self) -> ValueId {
        for (f, x, l) in std::mem::take(&mut self.defs) {
            self.cx.define_with(f, x, l);
        }
        if !self.defs.is_empty() || !self.own.is_empty() {
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
        if let Some(args) = self.leftover() {
            self.cx.child(self.ops.exec, args);
        }
        let mut n = self.pulled;
        if !self.eof && self.hint > i64::try_from(n).unwrap_or(0) {
            // re-emit the siblings the hint asks for, for the children
            let want = usize::try_from(self.hint).unwrap_or(0);
            while n < want {
                match self.cx.following(n) {
                    Some((op, args)) if op == self.ops.exec => {
                        self.cx.child(op, args);
                        n += 1;
                    }
                    _ => break,
                }
            }
        }
        self.cx.claim(n);
        self.cx.set_effect(self.effect);
        ValueId::UNIT
    }

    /// Ends a run that cannot finish: nothing defined but `ended` (so that
    /// nothing after it runs on a wrong state), and the need, for the
    /// client.
    pub(crate) fn stall(self, need: &str, what: ValueId) -> ValueId {
        let v = self.v;
        // nothing after a stalled command runs until it is answered
        self.cx.define_with(v.field("ended"), v.int(2), Locality::Global);
        let id = v.int(i64::from_ne_bytes(self.cx.node().raw().to_ne_bytes()));
        let e = v.list(vec![v.str(need), id, what]);
        self.cx.contribute(v.field("needs"), v.seq_unit(e));
        self.cx.claim(0);
        self.cx.set_effect(Effect::None);
        ValueId::UNIT
    }
}

fn is_hex(c: u32) -> bool {
    (u32::from(b'0')..=u32::from(b'9')).contains(&c) || (u32::from(b'a')..=u32::from(b'f')).contains(&c)
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
    let Value::List(fs) = &*v.get(l) else { return out };
    for &f in fs.iter() {
        let Value::List(xs) = &*v.get(f) else { continue };
        match xs.first().map(|&t| str_of(v, t)).as_deref() {
            Some("t") => {
                let toks = crate::val::toks_of(v, xs.get(1).copied());
                out.push(Frame::Toks(Rc::from(toks), 0));
            }
            Some("s") => {
                let text = xs.get(1).map(|&t| str_of(v, t)).unwrap_or_default();
                out.push(Frame::Src(Src {
                    buf: text.chars().map(u32::from).collect(),
                    loc: 0,
                    state: u8::try_from(xs.get(2).map_or(2, |&x| int_of(v, x))).unwrap_or(2),
                    endch: xs.get(3).map_or(-2, |&x| int_of(v, x)),
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
    let frame = v.list(vec![v.str("s"), v.str(&text), v.int(i64::from(NEW_LINE)), v.int(-2)]);
    vec![v.list(vec![frame])]
}
