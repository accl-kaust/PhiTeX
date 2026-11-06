//! A picture as a child node of its figure's run
//! (docs/spec-picture-child.md: the paragraph-node design's opaque
//! environment).
//!
//! A figure's float is one node: the run opens the float's box and reads
//! on until it closes, so the picture inside (`\begin{tikzpicture}` to
//! `\end{tikzpicture}`, a second of pgfplots) runs again whenever the
//! float's own reads move (`\@freelist`, its box register, the font list
//! microtype keeps). Here the run of an edit's evaluation places the
//! picture's text as a child node of its own ([`Pic`], `Ctx::child`) and
//! takes the child's output in its place ([`Run::pic_begin`]): the items
//! the picture appended, the list's parameters after it, the names it
//! defined globally and the names it read. The child re-runs only when
//! its own text or what it itself read changes.
//!
//! The child stands after the figure's run in text order, but in TeX its
//! commands ran in the middle of the figure's. So it reads every
//! definition before its parent (`Ctx::read_before_placer`), and what
//! the parent had bound up to the picture from the parent itself (the
//! `pic:` field, [`Run::pic_local_read`]): never what the parent defines
//! after the picture. And it publishes and contributes nothing: its
//! definitions (global and base-level local), its contributions (log
//! lines, writes, font markers, file reads) and the document lines it
//! began are in its output, and the parent makes them its own where the
//! picture stands ([`Run::pic_apply`]), so what the parent reads or
//! defines after the picture comes after them, as in TeX, and no reader
//! of the engine sees them anywhere else. A child that would reach past
//! its own text (an unbalanced group, a cut, a look at the list before
//! it, children of its own) refuses: the parent runs the picture inline
//! and never places it again in this process. The table of
//! environments ([`PICS`]) and the text match are hints: a wrong match
//! costs one run of the figure, never the output.
//!
//! A run's first pass over a figure runs the picture inline and places
//! the child beside it; the next run takes the output. A cold build's
//! runs place nothing ([`Run::run_all`] arms the trigger only in an
//! edit's evaluation).

use std::hash::{Hash as _, Hasher as _};

use phitex_defuse::{Ctx, Effect, Field, Locality, Operation, Step, Stop, ValueId, View};

use crate::build::FoldState;
use crate::input::{Input, MID_LINE, Seg, kind};
use crate::nest::{Item, Level};
use crate::ops::{Shared, ops, run_node};
use crate::run::{FileLine, Halt, R, Run};
use crate::state::{Entry, Loc};
use crate::val::{Tok, int_of, list_of, toks_of, toks_value};

/// The box environments a run places as children: their names, as
/// `\begin{name}` names them. Pictures do not nest.
const PICS: &[&str] = &["tikzpicture", "pgfpicture"];

/// How many lines ahead a scan for the end looks, at most.
const SCAN_LINES: usize = 20_000;

/// The `\begin` token.
pub(crate) fn begin_tok() -> Tok {
    static T: std::sync::OnceLock<Tok> = std::sync::OnceLock::new();
    *T.get_or_init(|| Tok::cs("begin"))
}

fn end_tok() -> Tok {
    static T: std::sync::OnceLock<Tok> = std::sync::OnceLock::new();
    *T.get_or_init(|| Tok::cs("end"))
}

/// The pictures (by the hash of their text) a run may never place again
/// in this process: a check refused one, or its child refused its run.
static REFUSED: std::sync::Mutex<Option<phitex_defuse::FxHashSet<u64>>> =
    std::sync::Mutex::new(None);

fn refused(hash: u64) -> bool {
    REFUSED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .is_some_and(|s| s.contains(&hash))
}

fn refuse(hash: u64) {
    REFUSED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_or_insert_with(Default::default)
        .insert(hash);
}

/// What the child's run keeps ([`Run::pic_run`]).
pub(crate) struct PicChild {
    /// The parent's locals, a field read key by key
    /// ([`Run::pic_local_read`]).
    field: Field,
    view: Option<View>,
    /// The names read (from the locals or the engine), in order.
    read: Vec<ValueId>,
    /// Its base level's list: a look at what lies before the picture on
    /// it (its tail, empty here, is the parent's) refuses.
    pub lid: i64,
    pub peeked: bool,
    /// Its `\write`s, `\openout`s and `\closeout`s as the parent makes
    /// them ([`Run::contribute_stream`]).
    pub streams: Vec<ValueId>,
}

/// How the parent's input skips the body once the child's output is taken.
enum Skip {
    /// `n` tokens of the top token list (the `\begin` token read already).
    Toks(usize),
    /// The current line, to index `k`.
    Here(usize),
    /// The current line to its end, `n` lines of the file below it, and
    /// the next line to index `k`.
    File { n: usize, k: usize },
    /// The current line to its end, the text's segments to `j`, and
    /// segment `j` (a line) to index `k`.
    Text { j: usize, k: usize },
}

/// A picture's body found at `\begin` ([`Run::pic_find`]).
struct Body {
    name: &'static str,
    /// The child's text: the `\begin` token, then the body to the end
    /// of `\end{name}`.
    segs: Vec<Seg>,
    skip: Skip,
    hash: u64,
}

/// `{name}` at the start of `toks`, `name` one of [`PICS`]: the name and
/// the tokens it took.
fn name_in_toks(toks: &[Tok]) -> Option<(&'static str, usize)> {
    if toks.first()?.cat()? != 1 {
        return None;
    }
    let mut s = String::new();
    for (i, t) in toks.iter().enumerate().skip(1) {
        match t.cat()? {
            2 => {
                let name = PICS.iter().find(|&&p| p == s)?;
                return Some((name, i + 1));
            }
            11 | 12 => s.push(char::from_u32(t.chr())?),
            _ => return None,
        }
        if s.len() > 16 {
            return None;
        }
    }
    None
}

/// `{name}` at `buf[from..]` (blanks before it skipped), `name` one of
/// [`PICS`]: the name and the index after `}`.
fn name_in_chars(buf: &[u32], from: usize) -> Option<(&'static str, usize)> {
    let mut i = from;
    while buf.get(i) == Some(&32) {
        i += 1;
    }
    if buf.get(i) != Some(&u32::from(b'{')) {
        return None;
    }
    let mut s = String::new();
    for (j, &c) in buf.iter().enumerate().skip(i + 1) {
        if c == u32::from(b'}') {
            let name = PICS.iter().find(|&&p| p == s)?;
            return Some((name, j + 1));
        }
        s.push(char::from_u32(c)?);
        if s.len() > 16 {
            return None;
        }
    }
    None
}

/// `\end{name}` in `buf[from..]`: the index after its `}`.
fn end_in_chars(buf: &[u32], from: usize, name: &str) -> Option<usize> {
    let pat: Vec<u32> = "\\end".chars().map(u32::from).collect();
    let mut i = from;
    while i + pat.len() <= buf.len() {
        if buf[i..i + pat.len()] == pat[..]
            && let Some((n, k)) = name_in_chars(buf, i + pat.len())
            && n == name
        {
            return Some(k);
        }
        i += 1;
    }
    None
}

/// `\end{name}` in `toks[from..]`: the index after its `}`.
fn end_in_toks(toks: &[Tok], from: usize, name: &str) -> Option<usize> {
    let end = end_tok();
    (from..toks.len()).find_map(|i| {
        if toks[i] != end {
            return None;
        }
        let (n, k) = name_in_toks(&toks[i + 1..])?;
        (n == name).then_some(i + 1 + k)
    })
}

/// A hash of the body's text (the child's identity in its field's name).
fn hash_segs(segs: &[Seg]) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    for s in segs {
        match s {
            Seg::Toks(ts) => {
                1u8.hash(&mut h);
                for t in ts {
                    match t.as_cs() {
                        Some(n) => {
                            let n: &str = &n;
                            n.hash(&mut h);
                        }
                        None => t.raw().hash(&mut h),
                    }
                }
            }
            Seg::Line(l) => {
                2u8.hash(&mut h);
                l.buf[l.loc.min(l.buf.len())..].hash(&mut h);
                l.ends.hash(&mut h);
            }
            Seg::File(_) => 3u8.hash(&mut h),
        }
    }
    h.finish()
}

fn some(x: ValueId) -> Option<ValueId> {
    (x != ValueId::UNIT).then_some(x)
}

fn opt(x: Option<ValueId>) -> ValueId {
    x.unwrap_or(ValueId::UNIT)
}

fn trace() -> bool {
    crate::exec::obs_flag!("PHITEX_PIC_TRACE")
}

impl Run<'_, '_> {
    /// At `\begin` read by the command loop: whether the picture's body
    /// was taken from the input and the child's output put in its place
    /// (true), or the run goes on inline (false: no picture here, a
    /// first run, a picture refused before, or no output yet; the child
    /// placed where it was not before).
    pub(crate) fn pic_begin(&mut self) -> bool {
        if self.interp.is_some() || self.in_skip || self.mode_zero || self.nest.is_empty() {
            return false;
        }
        let Some(body) = self.pic_find() else {
            return false;
        };
        let hash = body.hash;
        let node = self.cx.node().raw();
        if refused(hash) {
            if trace() {
                eprintln!(
                    "pic: node {node} {} {hash:x}: refused before, inline",
                    body.name
                );
            }
            return false;
        }
        if !self.cx.ran_before() {
            if trace() {
                eprintln!(
                    "pic: node {node} {} {hash:x}: the node's first run, inline",
                    body.name
                );
            }
            return false;
        }
        let v = self.v;
        // (the field's name: the body's text and its place among this
        // run's pictures of that text, never the run's position)
        let k = self.pic_mark_k(hash);
        let field = v.field(&format!("pic:{hash:x}:{k}"));
        let locals = self.pic_locals();
        self.cx.define_with(field, locals, Locality::Global);
        let frames = self.pic_frames(&body);
        let params = self.pic_params();
        let args = vec![
            frames,
            field.0,
            params,
            v.int(i64::from_ne_bytes(hash.to_ne_bytes())),
        ];
        let i = self.cx.child_once(self.ops.pic, args);
        self.muts += 1;
        let Some(x) = self.cx.placed_output_peek(i) else {
            // (no output yet: this run runs the picture inline, and the
            // child's commit asks it again)
            self.cx.placed_output_or(i, ValueId::UNIT);
            if trace() {
                eprintln!(
                    "pic: node {node} {} {hash:x}: placed, no output yet, inline",
                    body.name
                );
            }
            return false;
        };
        let xs = list_of(v, x);
        if x == ValueId::UNIT || xs.first().is_none_or(|&t| int_of(v, t) != 0) {
            refuse(hash);
            if trace() {
                eprintln!(
                    "pic: node {node} {} {hash:x}: the child refused ({}), inline from now on",
                    body.name,
                    xs.get(1).map(|&w| v.render(w)).unwrap_or_default()
                );
            }
            return false;
        }
        let x = self.cx.placed_output_or(i, ValueId::UNIT);
        self.pic_apply(&body, x);
        if trace() {
            eprintln!(
                "pic: node {node} {} {hash:x}: the child's output taken ({} items)",
                body.name,
                xs.get(1).map_or(0, |&l| list_of(v, l).len())
            );
        }
        true
    }

    /// The ordinal of a picture of text `hash` among this run's.
    fn pic_mark_k(&mut self, hash: u64) -> u32 {
        // (kept in the run's touched-field set: a run has a few pictures)
        let k = self.pic_seen.iter().filter(|&&h| h == hash).count();
        self.pic_seen.push(hash);
        u32::try_from(k).unwrap_or(0)
    }

    /// The picture's body at the input's top: the `{name}` after the
    /// `\begin` just read, then `\end{name}` further on in the same
    /// token list, line, file or text.
    fn pic_find(&self) -> Option<Body> {
        let top = *self.input.levels.last()?;
        match top.kind {
            kind::TOKS => self.pic_find_toks(top),
            kind::LINE => self.pic_find_lines(top),
            _ => None,
        }
    }

    fn pic_find_toks(&self, top: crate::input::Level) -> Option<Body> {
        let toks = self.input.arena.get(top.rest());
        let (name, nlen) = name_in_toks(toks)?;
        let n = end_in_toks(toks, nlen, name)?;
        let mut body = Vec::with_capacity(n + 1);
        body.push(self.cur.tok);
        body.extend_from_slice(&toks[..n]);
        let segs = vec![Seg::Toks(body)];
        let hash = hash_segs(&segs);
        Some(Body {
            name,
            segs,
            skip: Skip::Toks(n),
            hash,
        })
    }

    fn pic_find_lines(&self, top: crate::input::Level) -> Option<Body> {
        let l = self.input.line(top);
        if l.rescanned {
            return None;
        }
        let (name, after) = name_in_chars(&l.buf, l.loc)?;
        let mut segs = vec![Seg::Toks(vec![self.cur.tok])];
        // (the end on this line: the body is the line to it)
        if let Some(k) = end_in_chars(&l.buf, after, name) {
            let mut first = l.clone();
            first.buf.truncate(k);
            first.ends = false;
            segs.push(Seg::Line(first));
            let hash = hash_segs(&segs);
            return Some(Body {
                name,
                segs,
                skip: Skip::Here(k),
                hash,
            });
        }
        segs.push(Seg::Line(l.clone()));
        // (the lines after it: the file read below this line, or the
        // run's text where no level below has input left)
        let n = self.input.levels.len();
        let below = self.input.levels[..n - 1]
            .iter()
            .rev()
            .find(|&&l| match l.kind {
                kind::TOKS => l.a < l.b,
                kind::LINE => self.input.line(l).has_more(),
                _ => self.input.file(l).has_lines(),
            });
        match below {
            Some(lv) if lv.kind == kind::FILE => {
                let f = self.input.file(*lv);
                if f.lazy() {
                    return None;
                }
                let (mut off, mut pos, mut line, len) = f.scan_from();
                let mut n = 0;
                while off < len && n < SCAN_LINES {
                    let (ln, off2, pos2, line2) = f.line_at(off, pos, line);
                    if let Some(k) = end_in_chars(&ln.buf, 0, name) {
                        let mut e = ln;
                        e.buf.truncate(k);
                        e.ends = false;
                        segs.push(Seg::Line(e));
                        let hash = hash_segs(&segs);
                        return Some(Body {
                            name,
                            segs,
                            skip: Skip::File { n, k },
                            hash,
                        });
                    }
                    segs.push(Seg::Line(ln));
                    (off, pos, line) = (off2, pos2, line2);
                    n += 1;
                }
                None
            }
            Some(_) => None,
            None => {
                let at = self.input.text_at;
                for (j, s) in self.input.text.iter().enumerate().skip(at) {
                    let Seg::Line(ln) = s else { return None };
                    if let Some(k) = end_in_chars(&ln.buf, 0, name) {
                        let mut e = ln.clone();
                        e.buf.truncate(k);
                        e.ends = false;
                        segs.push(Seg::Line(e));
                        let hash = hash_segs(&segs);
                        return Some(Body {
                            name,
                            segs,
                            skip: Skip::Text { j, k },
                            hash,
                        });
                    }
                    segs.push(Seg::Line(ln.clone()));
                    if j - at > SCAN_LINES {
                        return None;
                    }
                }
                None
            }
        }
    }

    /// The body's segments as a node's text (an exec node's first
    /// argument): a token frame with this place's file line, the lines
    /// as they are ([`Run::leftover`]).
    fn pic_frames(&self, body: &Body) -> ValueId {
        let v = self.v;
        let here: Vec<ValueId> = match self.file_line_here() {
            Some(FileLine::Known(n)) => vec![ValueId::UNIT, v.int(n)],
            Some(FileLine::Piece(name, own)) => crate::run::piece_here(v, name, own),
            None => Vec::new(),
        };
        let mut frames = Vec::with_capacity(body.segs.len());
        for s in &body.segs {
            match s {
                Seg::Toks(t) => {
                    let mut x = vec![v.str("t"), toks_value(v, t)];
                    x.extend(here.iter().copied());
                    frames.push(v.list(x));
                }
                Seg::Line(l) => frames.extend(crate::pages::line_frame(v, l)),
                Seg::File(_) => {}
            }
        }
        v.list(frames)
    }

    /// Every binding the run made so far, as it stands, by name: what
    /// the child reads first ([`Run::pic_local_read`]). What the run only
    /// read the child reads from the engine before the run, where the run
    /// read it; what the run defines after the picture it never sees.
    fn pic_locals(&mut self) -> ValueId {
        let v = self.v;
        self.flush_bulk_codes();
        let now = self.st.defined_now(v);
        let mut xs = Vec::with_capacity(now.len());
        for (loc, cur) in now {
            let f = self.loc_field(loc);
            xs.push(v.list(vec![f.0, opt(cur)]));
        }
        v.list(xs)
    }

    /// The list the picture is appended to, as it stands: its mode and
    /// parameters; the run's alignment state, conditionals, XeTeX's
    /// inter-character class, the groups open, the level's extra.
    fn pic_params(&self) -> ValueId {
        let v = self.v;
        let t = self.nest.last().expect("a level");
        let (lang, packed) = t.par.map_or((ValueId::UNIT, ValueId::UNIT), |(a, b)| {
            (v.int(a), v.int(b))
        });
        v.list(vec![
            v.int(i64::from(t.mode)),
            v.int(i64::from(t.fold.sf)),
            v.int(i64::from(t.fold.pd)),
            lang,
            packed,
            v.int(t.clang),
            v.int(t.prev_graf),
            self.astate.map_or(ValueId::UNIT, |a| v.int(a)),
            v.int(self.aopen),
            v.list(self.conds.iter().map(|&c| v.int(i64::from(c))).collect()),
            v.list(self.if_codes.iter().map(|&c| v.int(i64::from(c))).collect()),
            v.int(i64::from(self.ic_prev)),
            v.int(self.local_groups()),
            v.list(t.extra.clone()),
        ])
    }

    /// The child's output in the run: the body skipped in the input, its
    /// items appended, the list's parameters set, the paragraph it began
    /// pushed, its base-level locals defined here, the run's transient
    /// state as the child left it.
    fn pic_apply(&mut self, body: &Body, x: ValueId) {
        let v = self.v;
        self.muts += 1;
        self.pic_skip(&body.skip);
        let xs = list_of(v, x);
        let a = |i: usize| xs.get(i).copied().unwrap_or(ValueId::UNIT);
        let n = |i: usize| int_of(v, a(i));
        if let Some(t) = self.nest.last_mut() {
            t.list.extend(list_of(v, a(1)).into_iter().map(Item::V));
            t.fold.sf = i32::try_from(n(2)).unwrap_or(1000);
            t.fold.pd = i32::try_from(n(3)).unwrap_or(t.fold.pd);
            t.fold.since_bp += usize::try_from(n(4)).unwrap_or(0);
            if a(5) != ValueId::UNIT {
                t.par = Some((n(5), n(6)));
            }
            t.clang = n(7);
            t.prev_graf = n(8);
            t.ag.extend(toks_of(v, Some(a(16))));
        }
        // (what the child contributed, the parent's where the picture
        // stands: its log lines among the parent's, its font markers and
        // writes before the parent's later ones; the lines it began)
        for e in list_of(v, a(17)) {
            if let [f, x] = list_of(v, e)[..] {
                self.contribs.push((Field(f), x));
            }
        }
        for e in list_of(v, a(18)) {
            self.contribute_stream(e);
        }
        for e in list_of(v, a(19)) {
            if let [n, h] = list_of(v, e)[..] {
                let name = crate::val::str_of(v, n);
                self.fread
                    .push((name, crate::val::int_of(v, h).cast_unsigned()));
            }
        }
        for e in list_of(v, a(20)) {
            if let [n, k] = list_of(v, e)[..] {
                let k = int_of(v, k);
                match self.doc_lines.iter_mut().find(|(m, _)| *m == n) {
                    Some((_, j)) => *j += k,
                    None => self.doc_lines.push((n, k)),
                }
            }
        }
        for e in list_of(v, a(10)) {
            let [name, val] = list_of(v, e)[..] else {
                continue;
            };
            let loc = self.loc_of(Field(name));
            let grp = self.innermost_group();
            self.st.set(loc, Entry::raw_seen(v, some(val)), false, grp);
        }
        if a(9) != ValueId::UNIT {
            let ys = list_of(v, a(9));
            let m = |i: usize| ys.get(i).map_or(0, |&y| int_of(v, y));
            let extra = ys.get(11).map(|&e| list_of(v, e)).unwrap_or_default();
            self.push_list(i32::try_from(m(0)).unwrap_or(102), 0, extra);
            if let Some(l) = self.top_mut() {
                l.list =
                    crate::nest::of_values(ys.get(1).map(|&i| list_of(v, i)).unwrap_or_default());
                l.fold = FoldState {
                    sf: i32::try_from(m(2)).unwrap_or(1000),
                    pd: i32::try_from(m(3)).unwrap_or(crate::build::IGNORE_DEPTH),
                    since_bp: usize::try_from(m(4)).unwrap_or(0),
                };
                l.par = (ys.get(5).copied() != Some(ValueId::UNIT)).then(|| (m(5), m(6)));
                l.clang = m(7);
                l.prev_graf = m(8);
                l.mode_line = ys.get(9).and_then(|&x| crate::resume::line_mark_of(v, x));
                l.mode_place = ys.get(10).and_then(|&x| crate::resume::place_of(v, x));
                l.ag = toks_of(v, ys.get(12).copied());
            }
        }
        self.astate = some(a(13)).map(|x| int_of(v, x));
        self.aopen = n(14);
        self.ic_prev = i32::try_from(n(15)).unwrap_or(self.ic_prev);
        // (its global definitions, made here, where TeX made them: the
        // run's own reads and definitions after the picture see them and
        // override them as TeX's do)
        for e in list_of(v, a(11)) {
            let [name, val] = list_of(v, e)[..] else {
                continue;
            };
            let loc = self.loc_of(Field(name));
            let grp = self.innermost_group();
            self.st.set(loc, Entry::raw_seen(v, some(val)), true, grp);
        }
    }

    /// The input moved past the body ([`Skip`]).
    fn pic_skip(&mut self, skip: &Skip) {
        match *skip {
            Skip::Toks(n) => {
                if let Some(l) = self.input.levels.last_mut() {
                    l.a =
                        l.a.saturating_add(u32::try_from(n).unwrap_or(u32::MAX))
                            .min(l.b);
                }
            }
            Skip::Here(k) => {
                if let Some(top) = self.input.levels.last().copied() {
                    let l = self.input.line_mut(top);
                    l.loc = k.min(l.buf.len());
                    l.state = MID_LINE;
                }
            }
            Skip::File { n, k } => {
                self.input.pop();
                self.input.pop_spent();
                let Some(top) = self.input.levels.last().copied() else {
                    return;
                };
                let f = self.input.file_mut(top);
                for _ in 0..n {
                    f.take_line();
                }
                let mut e = f.take_line();
                e.loc = k.min(e.buf.len());
                e.state = MID_LINE;
                e.starts = false;
                self.input.push_line(e);
            }
            Skip::Text { j, k } => {
                self.input.pop();
                self.input.pop_spent();
                self.input.text_at = j;
                if let Some(Seg::Line(e)) = self.input.text.get_mut(j) {
                    e.loc = k.min(e.buf.len());
                    e.state = MID_LINE;
                    e.starts = false;
                }
            }
        }
    }

    // ------------------------------------------------------- the child

    /// A read of the child's that misses its table: the parent's locals
    /// first, by name (a read of that key alone); true when found. Every
    /// name read is noted for the parent's check.
    pub(crate) fn pic_local_read(&mut self, loc: Loc, f: Field) -> R<bool> {
        let Some(p) = self.pic_child.as_mut() else {
            return Ok(false);
        };
        let view = if let Some(w) = p.view {
            w
        } else {
            let w = self.cx.view_field(p.field)?;
            p.view = Some(w);
            w
        };
        let Some(x) = self.cx.get(view, Step::Key(f.0)) else {
            // (read from the engine, before the parent)
            if let Some(p) = self.pic_child.as_mut() {
                p.read.push(f.0);
            }
            return Ok(false);
        };
        let x = some(x);
        self.st.load_read(self.v, loc, x);
        if self.st.load_state(loc) == 2 {
            self.unalias(loc)?;
        }
        self.note_loaded_base(loc, x);
        Ok(true)
    }

    /// The child's run: the body on a level of its own in the parent's
    /// mode, its locals from the field; at the end, the output
    /// ([`Run::pic_output`]) or a refusal ([`Run::pic_refuse`]).
    fn pic_run(&mut self, args: &[ValueId]) -> R<()> {
        let v = self.v;
        let a = |i: usize| args.get(i).copied().unwrap_or(ValueId::UNIT);
        let frames = self.resolve_units(&[a(0)])?;
        let fs = crate::run::decode_segs(v, &frames);
        let p = list_of(v, a(2));
        let n = |i: usize| p.get(i).map_or(0, |&x| int_of(v, x));
        let extra = p.get(13).map(|&x| list_of(v, x)).unwrap_or_default();
        let mode = i32::try_from(n(0)).unwrap_or(1);
        // (every read of the engine's before the parent: where TeX's
        // picture read, inside the parent's text)
        self.cx.read_before_placer();
        let lid = self.push_list(mode, 0, extra);
        if let Some(t) = self.top_mut() {
            t.fold = FoldState {
                sf: i32::try_from(n(1)).unwrap_or(1000),
                pd: i32::try_from(n(2)).unwrap_or(crate::build::IGNORE_DEPTH),
                since_bp: 0,
            };
            t.par = (p.get(3).copied() != Some(ValueId::UNIT)).then(|| (n(3), n(4)));
            t.clang = n(5);
            t.prev_graf = n(6);
        }
        self.astate = p.get(7).copied().and_then(some).map(|x| int_of(v, x));
        self.aopen = n(8);
        self.conds = list_of(v, a(2))
            .get(9)
            .map(|&c| {
                list_of(v, c)
                    .iter()
                    .map(|&x| u8::try_from(int_of(v, x)).unwrap_or(0))
                    .collect()
            })
            .unwrap_or_default();
        self.if_codes = p
            .get(10)
            .map(|&c| {
                list_of(v, c)
                    .iter()
                    .map(|&x| i32::try_from(int_of(v, x)).unwrap_or(0))
                    .collect()
            })
            .unwrap_or_default();
        self.ic_prev = i32::try_from(n(11)).unwrap_or(self.ic_prev);
        self.depth_add = n(12);
        self.eof = true;
        self.no_resume = true;
        self.pic_child = Some(PicChild {
            field: Field(a(1)),
            view: None,
            read: Vec::new(),
            lid,
            peeked: false,
            streams: Vec::new(),
        });
        let conds0 = (self.conds.clone(), self.if_codes.clone());
        self.input.append_text(fs);
        let res = self.run_all();
        let why = match res {
            Err(Halt::Stop(s)) => return Err(Halt::Stop(s)),
            Err(Halt::Starve) => Some("the body wanted input past its end".to_owned()),
            Err(Halt::Hole) => Some("a hole".to_owned()),
            Err(Halt::Fatal) => Some("a fatal error".to_owned()),
            Err(Halt::Cut | Halt::Recut(_)) => Some("a cut".to_owned()),
            Err(Halt::Same) => Some("a cutoff".to_owned()),
            Ok(()) => self.pic_shape(lid, &conds0),
        };
        if let Some(w) = why {
            self.pic_refuse(&w);
        } else {
            self.pic_out = Some(self.pic_output());
        }
        Ok(())
    }

    /// Why the child's run cannot stand as a picture's: anything of it
    /// that would reach past its own text.
    fn pic_shape(&mut self, lid: i64, conds0: &(Vec<u8>, Vec<i32>)) -> Option<String> {
        let v = self.v;
        if self.closed_scope() {
            return Some("it closed the scope".to_owned());
        }
        if self.claim_rest || self.fatal_hit {
            return Some("it ended the job".to_owned());
        }
        if self.effect != Effect::None {
            return Some(format!("a scope effect {:?}", self.effect));
        }
        let shape_ok = match self.nest[..] {
            [ref b] => b.owns && b.group == 0 && b.lid == lid,
            [ref b, ref p] => b.owns && b.group == 0 && b.lid == lid && p.owns && p.group == 0,
            _ => false,
        };
        if !shape_ok {
            return Some(format!(
                "unbalanced: {} levels open, {:?}",
                self.nest.len(),
                self.nest
                    .iter()
                    .map(|l| (l.mode, l.group, l.owns))
                    .collect::<Vec<_>>()
            ));
        }
        if (self.conds.clone(), self.if_codes.clone()) != *conds0 {
            return Some("a conditional opened or closed".to_owned());
        }
        if self.cut.is_some() || self.close_cont.is_some() {
            return Some("a cut".to_owned());
        }
        if !self.kids.is_empty() {
            return Some("children of its own".to_owned());
        }
        if self.rt_made > 0 {
            return Some("read-time objects".to_owned());
        }
        if self.pic_child.as_ref().is_some_and(|p| p.peeked) {
            return Some("it looked at the list before it".to_owned());
        }
        // (an item for the main list other than a marker: the picture
        // was not inside a box)
        let items = v.field("items");
        for (f, x) in &self.contribs {
            if *f == items
                && v.seq_to_vec(*x).into_iter().any(|e| {
                    list_of(v, e)
                        .get(1)
                        .is_none_or(|&n| !crate::build::is_marker(v, n))
                })
            {
                return Some("items for a list of the engine's".to_owned());
            }
        }
        if !self.leftover().is_empty() {
            return Some("input left".to_owned());
        }
        for (_, w, l) in self.publication() {
            if matches!(w, crate::state::Pub::Rel(_)) {
                return Some("a relation defined".to_owned());
            }
            if !matches!(l, Locality::Local | Locality::Global) {
                return Some("a scope's definition".to_owned());
            }
        }
        None
    }

    /// The child's run refused: nothing of it reaches the engine, its
    /// output says why.
    fn pic_refuse(&mut self, why: &str) {
        let v = self.v;
        if trace() {
            eprintln!("pic: child {} refuses: {why}", self.cx.node().raw());
        }
        self.pic_void = true;
        self.muts += 1;
        self.st = crate::state::State::new();
        self.pubs = None;
        self.contribs.clear();
        self.kids.clear();
        self.fread.clear();
        self.doc_lines.clear();
        self.nest.clear();
        self.effect = Effect::None;
        self.claim_rest = false;
        self.fatal_hit = false;
        self.cut = None;
        self.close_cont = None;
        self.input = Input::new();
        self.conds.clear();
        self.if_codes.clear();
        self.cond_stop = false;
        self.cond_loaded = false;
        if let Some(p) = self.pic_child.as_mut() {
            p.streams.clear();
        }
        self.pic_out = Some(v.list(vec![v.int(1), v.str(why)]));
    }

    /// The child's output: `[0, items, sf, pd, since_bp, lang, packed,
    /// clang, prevgraf, pushed level, locals, global names, read names,
    /// astate, aopen, ic_prev, aftergroup, contributions, streams, file
    /// reads, document lines begun]`; the run itself then contributes
    /// nothing.
    fn pic_output(&mut self) -> ValueId {
        let v = self.v;
        let contribs: Vec<ValueId> = std::mem::take(&mut self.contribs)
            .into_iter()
            .map(|(f, x)| v.list(vec![f.0, x]))
            .collect();
        let streams = self
            .pic_child
            .as_mut()
            .map(|p| std::mem::take(&mut p.streams))
            .unwrap_or_default();
        let fread: Vec<ValueId> = std::mem::take(&mut self.fread)
            .into_iter()
            .map(|(n, h)| v.list(vec![v.str(&n), crate::val::int_id(v, h.cast_signed())]))
            .collect();
        let lines: Vec<ValueId> = std::mem::take(&mut self.doc_lines)
            .into_iter()
            .map(|(n, k)| v.list(vec![n, v.int(k)]))
            .collect();
        let mut locals = Vec::new();
        let mut gdefs = Vec::new();
        for (f, x, l) in self.published_values() {
            match l {
                Locality::Local => locals.push(v.list(vec![f.0, x])),
                _ => gdefs.push(v.list(vec![f.0, x])),
            }
        }
        let mut reads: Vec<ValueId> = self
            .pic_child
            .as_ref()
            .map(|p| p.read.clone())
            .unwrap_or_default();
        reads.sort_unstable_by_key(|x| x.raw());
        reads.dedup();
        let mut nest = std::mem::take(&mut self.nest);
        let pushed = (nest.len() > 1).then(|| nest.pop()).flatten();
        let base = nest.remove(0);
        let level = |l: Level| -> ValueId {
            let (lang, packed) = l.par.map_or((ValueId::UNIT, ValueId::UNIT), |(a, b)| {
                (v.int(a), v.int(b))
            });
            let items = v.list(crate::nest::into_values(v, l.list));
            v.list(vec![
                v.int(i64::from(l.mode)),
                items,
                v.int(i64::from(l.fold.sf)),
                v.int(i64::from(l.fold.pd)),
                v.int(i64::try_from(l.fold.since_bp).unwrap_or(0)),
                lang,
                packed,
                v.int(l.clang),
                v.int(l.prev_graf),
                crate::resume::line_mark_value(v, l.mode_line),
                crate::resume::place_value(v, l.mode_place.as_ref()),
                v.list(l.extra),
                toks_value(v, &l.ag),
            ])
        };
        let (lang, packed) = base.par.map_or((ValueId::UNIT, ValueId::UNIT), |(a, b)| {
            (v.int(a), v.int(b))
        });
        let ag = toks_value(v, &base.ag);
        let items = v.list(crate::nest::into_values(v, base.list));
        v.list(vec![
            v.int(0),
            items,
            v.int(i64::from(base.fold.sf)),
            v.int(i64::from(base.fold.pd)),
            v.int(i64::try_from(base.fold.since_bp).unwrap_or(0)),
            lang,
            packed,
            v.int(base.clang),
            v.int(base.prev_graf),
            pushed.map_or(ValueId::UNIT, level),
            v.list(locals),
            v.list(gdefs),
            v.list(reads),
            self.astate.map_or(ValueId::UNIT, |a| v.int(a)),
            v.int(self.aopen),
            v.int(i64::from(self.ic_prev)),
            ag,
            v.list(contribs),
            v.list(streams),
            v.list(fread),
            v.list(lines),
        ])
    }
}

/// A picture's child node: arguments `[frames, locals field, params,
/// hash]`, the body's text as an exec node's, the parent's locals'
/// field, the list it is appended to as it stands, the text's hash.
pub(crate) struct Pic(pub Shared);

impl Operation for Pic {
    fn name(&self) -> &'static str {
        "pic"
    }
    /// Each runs its own text.
    fn alike(&self) -> bool {
        false
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new("pic", cx.node().raw());
        let _busy = crate::exec::Busy::new(&ops(&self.0).watch, 1, cx.node().raw());
        let args = cx.args().to_vec();
        let o = ops(&self.0);
        let t0 = std::time::Instant::now();
        let out = run_node(cx, o, |r| r.pic_run(&args));
        if trace() {
            let v = cx.values();
            eprintln!(
                "pic: child {} ran in {:.1} ms: {}",
                cx.node().raw(),
                t0.elapsed().as_secs_f64() * 1e3,
                out.as_ref().map_or_else(
                    |_| "stop".to_owned(),
                    |&x| v.render(x).chars().take(80).collect::<String>()
                )
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_and_the_end_are_found_in_characters() {
        let buf: Vec<u32> = "\\begin {tikzpicture}[x=1cm] \\end{tikzpicture}% rest"
            .chars()
            .map(u32::from)
            .collect();
        let (name, after) = name_in_chars(&buf, 6).expect("a name");
        assert_eq!(name, "tikzpicture");
        assert_eq!(after, 20);
        let k = end_in_chars(&buf, after, name).expect("an end");
        assert_eq!(
            &buf[k..k + 6]
                .iter()
                .map(|&c| char::from_u32(c).unwrap())
                .collect::<String>(),
            "% rest"
        );
        assert!(end_in_chars(&buf, after, "pgfpicture").is_none());
        assert!(name_in_chars(&buf, 0).is_none());
    }

    #[test]
    fn the_name_and_the_end_are_found_in_tokens() {
        let mut toks = vec![Tok::ch(1, u32::from(b'{'))];
        toks.extend("pgfpicture".chars().map(|c| Tok::ch(11, u32::from(c))));
        toks.push(Tok::ch(2, u32::from(b'}')));
        let n0 = toks.len();
        toks.push(Tok::cs("draw"));
        toks.push(end_tok());
        toks.push(Tok::ch(1, u32::from(b'{')));
        toks.extend("pgfpicture".chars().map(|c| Tok::ch(11, u32::from(c))));
        toks.push(Tok::ch(2, u32::from(b'}')));
        let n1 = toks.len();
        toks.push(Tok::ch(2, u32::from(b'}')));
        assert_eq!(name_in_toks(&toks), Some(("pgfpicture", n0)));
        assert_eq!(end_in_toks(&toks, n0, "pgfpicture"), Some(n1));
        assert_eq!(end_in_toks(&toks, n0, "tikzpicture"), None);
    }
}
