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
//! A run's first pass over a figure places the child and goes on as if
//! the picture were empty; the child's commit asks it again, and that
//! run takes the output. So every picture runs once, as a node of its
//! own, in a cold build too, beside the other nodes at more than one
//! thread.

use std::hash::{Hash as _, Hasher as _};

use phitex_defuse::{Ctx, Effect, Field, Locality, Operation, Step, Stop, ValueId, View};

use crate::build::FoldState;
use crate::input::{Input, MID_LINE, Seg, kind};
use crate::nest::{Item, Level};
use crate::ops::{Shared, ops, run_node};
use crate::run::{FileLine, Halt, R, Run};
use crate::state::Entry;
use crate::val::{Tok, int_of, list_of, toks_of, toks_value};

/// The box environments a run places as children: their names, as
/// `\begin{name}` names them. Pictures do not nest.
const PICS: &[&str] = &["tikzpicture", "pgfpicture"];

/// The key of the parent's bindings ([`Run::pic_locals`]) holding its
/// appends pending at the picture (`Run::appending`).
const APPENDING: &str = "\u{1}appending";

/// The same for the list the picture is appended to, as it stands
/// (`[items, since_bp]`): what a look past the start of the child's own
/// list sees ([`Run::pic_take_list`]).
const PARENT_LIST: &str = "\u{1}list";

/// The same for its values appended to lists (`Run::appending_vals`).
const APPENDING_VALS: &str = "\u{1}appending values";

/// The same for the registers it stepped without reading them
/// (`State::mapped_now`): `[field, steps, global]` each. The child starts
/// from those steps, so a symbol it makes of one (the register reaching
/// the parent plus steps, `Run::sym_node`) counts them, and its output
/// carries only its own ([`Run::pic_steps`]).
const PARENT_STEPS: &str = "\u{1}steps";

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
    /// (`Ctx::read_before_placer_over`).
    field: Field,
    view: Option<View>,
    /// Its base level's list, and how many of the parent's last items
    /// before the picture it holds in front of its own: each look past
    /// the start of its list takes one more ([`Run::pic_take_list`]), and
    /// its output's list then stands for those and its own.
    pub lid: i64,
    pub took: usize,
    /// Its `\write`s, `\openout`s and `\closeout`s as the parent makes
    /// them ([`Run::contribute_stream`]).
    pub streams: Vec<ValueId>,
    /// The parent's steps it started from ([`PARENT_STEPS`]).
    /// By field: a location past the fixed ones is the state's spill's,
    /// which a rollback renumbers.
    base_steps: Vec<(Field, i32)>,
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

/// Where the body's text starts in its file (its first line's origin's
/// file and the byte at its column), when not at the file's start: the
/// child's text gives its lines' places from there (one past it), so an
/// edit before the picture that moves it leaves the child's arguments
/// as they were and it runs no more (a typed word before a picture in
/// its paragraph ran it again: its lines' bytes in the document moved).
/// Its diagnostics' places are the file's again in the parent,
/// [`rebase_diags`].
fn pic_base(body: &Body) -> Option<(ValueId, i64)> {
    let (name, at, l) = body.segs.iter().find_map(|s| match s {
        Seg::Line(l) => l.origin.map(|(n, at, _)| (n, at, l)),
        _ => None,
    })?;
    let at = i64::try_from(at + l.loc.min(l.buf.len())).ok()?;
    (at > 0).then_some((name, at))
}

/// A line's frame ([`crate::pages::line_frame`]: `["s", text, state,
/// -2, flags, file, at, ..]`) with its place `at` from `base`'s (one
/// past it: a place 0 is a file's first line's, [`Run::xe_decode_top`]).
fn rebase_frame(v: &phitex_defuse::Values, f: ValueId, base: Option<(ValueId, i64)>) -> ValueId {
    let Some((name, base)) = base else {
        return f;
    };
    let mut xs = list_of(v, f);
    if xs.len() > 6 && xs[5] == name {
        let at = int_of(v, xs[6]);
        if at >= base {
            xs[6] = v.int(at - base + 1);
            return v.list(xs);
        }
    }
    f
}

/// The child's diagnostics (`diags`' sequence of `[kind, message, file,
/// at, ..]`, [`Run::diag_at`]) with the places in its body's text the
/// file's again ([`rebase_frame`] undone).
fn rebase_diags(v: &phitex_defuse::Values, x: ValueId, base: Option<(ValueId, i64)>) -> ValueId {
    let Some((name, base)) = base else {
        return x;
    };
    let es = v.seq_to_vec(x);
    if es.is_empty() {
        return x;
    }
    let mut out = ValueId::NIL;
    for e in es {
        let mut xs = list_of(v, e);
        let e = if xs.len() > 3 && xs[2] == name && int_of(v, xs[3]) >= 1 {
            xs[3] = v.int(int_of(v, xs[3]) + base - 1);
            v.list(xs)
        } else {
            e
        };
        out = v.seq_concat(out, e);
    }
    out
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
    /// Whether `cur` is a picture's `\begin` this run places as a child
    /// ([`Run::pic_begin`] says whether it does).
    pub(crate) fn pic_here(&self) -> bool {
        self.pic_on
            && (crate::val::cmd::CALL..=crate::val::cmd::LONG_OUTER_CALL).contains(&self.cur.cmd)
            && self.cur.tok == begin_tok()
    }

    /// Whether a picture at this `\\begin` is read inline whatever its
    /// body: a list of the run's own to put its items on is wanted (on the
    /// engine's main list they are contributions, `Run::pic_shape`).
    fn pic_not_here(&self) -> bool {
        let not = self.interp.is_some()
            || self.in_skip
            || self.mode_zero
            || !self.nest.iter().any(|l| l.owns);
        if not && trace() {
            eprintln!(
                "pic: node {} at \\begin: not here (interp {}, skip {}, mode zero {}, a list of its own {})",
                self.cx.node().raw(),
                self.interp.is_some(),
                self.in_skip,
                self.mode_zero,
                self.nest.iter().any(|l| l.owns)
            );
        }
        not
    }

    pub(crate) fn pic_begin(&mut self) -> R<bool> {
        if self.pic_not_here() {
            return Ok(false);
        }
        // (the `\begin` read from a token list it was put back in, a run
        // of characters' next command: the list it emptied gone, the
        // body read from the input below it)
        self.input.pop_spent();
        // (a body running on into the siblings' text (a paragraph that
        // goes on past a display, the picture's text a slice of its own):
        // their text claimed now, as reading the picture inline would
        // claim it at once)
        let mut found = self.pic_find();
        let mut pulls = 0;
        while found.is_none() && pulls < 64 && self.pic_reads_on() {
            if !self.pull()? {
                break;
            }
            pulls += 1;
            found = self.pic_find();
        }
        let Some(body) = found else {
            if trace() {
                self.pic_no_body_trace();
            }
            return Ok(false);
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
            return Ok(false);
        }
        let v = self.v;
        // (the field's name: the body's text and its place among this
        // run's pictures of that text, never the run's position)
        let k = self.pic_mark_k(hash);
        let field = v.field(&format!("pic:{hash:x}:{k}"));
        let locals = self.pic_locals();
        // (the parent's view the child has run on: this run's, where my
        // last run gave the child the same one; another, and the child's
        // output is of another state than this one)
        let view_held = self.cx.last_def(field) == Some(locals);
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
        // (a child whose view moved since its output: the picture read
        // here, inline, as TeX reads it, its output not read now; the child
        // runs on the view this run gave it, and the runs after take its
        // output. Taken, the pictures after it were given a state made with
        // that old output, their children ran again on it, and the run went
        // again for each one's new output, one picture a pass: lie-hasse's
        // page of diagrams after a page's routine moved TikZ's counter,
        // a 45 s piece run once per picture, past 3000 s cold, accl 26269)
        // (and one that has yet to run again on it, re-asked or running on
        // another thread: its output is of an older view as well, at -j4
        // taken and the run asked again for each new one, 2503 s)
        let peeked = self.cx.placed_output_peek(i);
        if peeked.is_some() && !(view_held && self.cx.placed_output_settled(i).is_some()) {
            self.muts += 1;
            if trace() {
                eprintln!(
                    "pic: node {node} {} {hash:x}: its view moved, inline",
                    body.name
                );
            }
            return Ok(false);
        }
        let Some(x) = peeked else {
            // (no output yet: the run goes on on the output of the
            // picture its last run placed here, an edited picture's old
            // one, and the child's commit asks it again where its own
            // differs: the fonts, names and appends the picture made stand
            // meanwhile, where empty they went and every later reader of
            // them was asked again, then again when the child had run
            // (Grail's changed drawing: 198 paragraphs checked, the first
            // load of an 8 pt font gone for that while))
            // (a guess only on my first run of an evaluation, an edit's: run
            // again in it, the old output is one this evaluation has moved
            // already, and pictures guessed on, one after another, were
            // each placed anew and asked the run again with their real
            // output, one a pass: lie-hasse -j4, its piece of diagrams run
            // 25 times on new children 671, 701, 712, ...)
            if !self.cx.ran_this_evaluation() && self.pic_guess(i, &body) {
                return Ok(true);
            }
            // (none: the picture read here, inline, as TeX reads it, and
            // the child placed for the runs after this one, its output not
            // read now (exact by construction, it is this one; the edits
            // after take it). Gone on as if empty, the pictures after it
            // were given the state without this one's work, their children
            // ran on it, and the run went again for each one's output, one
            // more right each time: lie-hasse's page of 24 diagrams, a 55 s
            // piece run 24 times, past 3000 s cold where 1073 s, accl 26269)
            self.muts += 1;
            if trace() {
                eprintln!(
                    "pic: node {node} {} {hash:x}: placed, no output yet, inline",
                    body.name
                );
            }
            return Ok(false);
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
            return Ok(false);
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
        Ok(true)
    }

    /// [`Run::pic_begin`] for a child that has not run: the output of the
    /// picture this node's last run placed at index `i` applied as the
    /// guess, where there is one the child did not refuse. Whether it was.
    fn pic_guess(&mut self, i: usize, body: &Body) -> bool {
        let v = self.v;
        let Some(g) = self.cx.last_placed_output(i, self.ops.pic).filter(|&g| {
            let gs = list_of(v, g);
            g != ValueId::UNIT && gs.first().is_some_and(|&t| int_of(v, t) == 0)
        }) else {
            return false;
        };
        let x = self.cx.placed_output_or(i, g);
        self.pic_apply(body, x);
        if trace() {
            eprintln!(
                "pic: node {} {} {:x}: placed, no output yet, its last run's picture here taken",
                self.cx.node().raw(),
                body.name,
                body.hash
            );
        }
        true
    }

    /// `PHITEX_PIC_TRACE`: why a `\\begin` found no picture body here.
    fn pic_no_body_trace(&self) {
        let top = self.input.levels.last().map(|l| l.kind);
        let here: String = self
            .input
            .levels
            .last()
            .filter(|l| l.kind == kind::LINE)
            .map(|&l| {
                let ln = self.input.line(l);
                ln.buf
                    .iter()
                    .skip(ln.loc)
                    .take(30)
                    .filter_map(|&c| char::from_u32(c))
                    .collect()
            })
            .unwrap_or_default();
        let segs: Vec<String> = self
            .input
            .text
            .iter()
            .skip(self.input.text_at)
            .take(4)
            .map(|s| match s {
                Seg::Line(l) => format!(
                    "line {:?}",
                    l.buf
                        .iter()
                        .take(24)
                        .filter_map(|&c| char::from_u32(c))
                        .collect::<String>()
                ),
                Seg::Toks(t) => format!("toks {}", t.len()),
                Seg::File(_) => "file".to_owned(),
            })
            .collect();
        eprintln!(
            "pic: node {} at \\begin: no picture body found (input top {top:?}, levels {}, next {here:?}, text after {segs:?}, pulled {}, rescanned {})",
            self.cx.node().raw(),
            self.input.levels.len(),
            self.pulled,
            self.input
                .levels
                .last()
                .is_some_and(|&l| l.kind == kind::LINE && self.input.line(l).rescanned)
        );
    }

    /// Whether the picture's `\begin{name}` may have its body in the
    /// siblings' text not claimed yet: one line on the input, the node's
    /// text after it lines only (the end not among them, [`Run::pic_find`]).
    fn pic_reads_on(&self) -> bool {
        let [top] = self.input.levels[..] else {
            return false;
        };
        top.kind == kind::LINE
            && self
                .input
                .text
                .iter()
                .skip(self.input.text_at)
                .all(|s| matches!(s, Seg::Line(_)))
            && name_in_chars(&self.input.line(top).buf, self.input.line(top).loc).is_some()
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
                Seg::Line(l) => frames.extend(
                    crate::pages::line_frame(v, l).map(|f| rebase_frame(v, f, pic_base(body))),
                ),
                Seg::File(_) => {}
            }
        }
        v.list(frames)
    }

    /// Every binding the run made so far, as it stands, by name: what
    /// the child reads first (`Ctx::read_before_placer_over`). What the run only
    /// read the child reads from the engine before the run, where the run
    /// read it; what the run defines after the picture it never sees.
    fn pic_locals(&mut self) -> ValueId {
        let v = self.v;
        self.flush_bulk_codes();
        let now = self.st.defined_now(v);
        let mut xs = Vec::with_capacity(now.len() + 1);
        for (loc, cur) in now {
            let f = self.loc_field(loc);
            xs.push(v.list(vec![f.0, opt(cur)]));
        }
        if !self.appending.is_empty() {
            let pending: Vec<ValueId> = self
                .appending
                .clone()
                .into_iter()
                .map(|(l, t)| v.list(vec![self.loc_field(l).0, toks_value(v, &t)]))
                .collect();
            xs.push(v.list(vec![v.str(APPENDING), v.list(pending)]));
        }
        if let Some(t) = self.nest.iter().rev().find(|l| l.owns) {
            let items = crate::nest::into_values(v, t.list.clone());
            let since = v.int(i64::try_from(t.fold.since_bp).unwrap_or(0));
            xs.push(v.list(vec![v.str(PARENT_LIST), v.list(vec![v.list(items), since])]));
        }
        let mapped = self.st.mapped_now();
        if !mapped.is_empty() {
            let steps: Vec<ValueId> = mapped
                .into_iter()
                .map(|(l, c, g)| {
                    v.list(vec![
                        self.loc_field(l).0,
                        v.int(i64::from(c)),
                        v.int(i64::from(g)),
                    ])
                })
                .collect();
            xs.push(v.list(vec![v.str(PARENT_STEPS), v.list(steps)]));
        }
        if !self.appending_vals.is_empty() {
            let pending: Vec<ValueId> = self
                .appending_vals
                .clone()
                .into_iter()
                .map(|(l, xs)| v.list(vec![self.loc_field(l).0, v.list(xs)]))
                .collect();
            xs.push(v.list(vec![v.str(APPENDING_VALS), v.list(pending)]));
        }
        v.list(xs)
    }

    /// The list the picture is appended to, as it stands: its mode and
    /// parameters; the run's alignment state, conditionals, XeTeX's
    /// inter-character class, the groups open, the level's extra.
    fn pic_params(&self) -> ValueId {
        let v = self.v;
        // (the level whose list it is: a group's level shares the list
        // of the level below it)
        let t = self
            .nest
            .iter()
            .rev()
            .find(|l| l.owns)
            .expect("a list of the run's");
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
        if let Some(t) = self.nest.iter_mut().rev().find(|l| l.owns) {
            // (the child's list stands for the parent's last items it
            // took and its own, `Run::pic_take_list`)
            let since = usize::try_from(n(4)).unwrap_or(0);
            let took = usize::try_from(n(23)).unwrap_or(0).min(t.list.len());
            t.list.truncate(t.list.len() - took);
            t.fold.since_bp -= took.min(t.fold.since_bp);
            t.list.extend(list_of(v, a(1)).into_iter().map(Item::V));
            t.fold.since_bp += since;
            t.fold.sf = i32::try_from(n(2)).unwrap_or(1000);
            t.fold.pd = i32::try_from(n(3)).unwrap_or(t.fold.pd);
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
        // (its diagnostics' places in the body's text made the file's
        // again, [`pic_base`])
        let diags = v.field("diags");
        for e in list_of(v, a(17)) {
            if let [f, x] = list_of(v, e)[..] {
                let x = if Field(f) == diags {
                    rebase_diags(v, x, pic_base(body))
                } else {
                    x
                };
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
            // (a list the child made whole: what the parent had pending
            // for it is in its value)
            self.appending.retain(|x| x.0 != loc);
            self.appending_vals.retain(|x| x.0 != loc);
        }
        self.pic_take_appends(a(21), a(22));
        self.pic_take_steps(a(24));
    }

    /// The appends pending after the picture ([`Run::pic_apply`]): the
    /// parent's before it and the child's, which went on from them.
    fn pic_take_appends(&mut self, toks: ValueId, vals: ValueId) {
        let v = self.v;
        for e in list_of(v, toks) {
            if let [f, t] = list_of(v, e)[..] {
                let loc = self.loc_of(Field(f));
                let toks = toks_of(v, Some(t));
                match self.appending.iter_mut().find(|x| x.0 == loc) {
                    Some(x) => x.1 = toks,
                    None => self.appending.push((loc, toks)),
                }
            }
        }
        for e in list_of(v, vals) {
            if let [f, xs] = list_of(v, e)[..] {
                let loc = self.loc_of(Field(f));
                let xs = list_of(v, xs);
                match self.appending_vals.iter_mut().find(|x| x.0 == loc) {
                    Some(x) => x.1 = xs,
                    None => self.appending_vals.push((loc, xs)),
                }
            }
        }
        // (a list the parent holds itself: the appends made on it now, no
        // relation beside the parent's own value)
        let held: Vec<crate::state::Loc> = self
            .appending
            .iter()
            .map(|x| x.0)
            .chain(self.appending_vals.iter().map(|x| x.0))
            .filter(|&l| self.st.load_state(l) != 0)
            .collect();
        for l in held {
            let r = self.append_force(l).and_then(|()| self.vals_force(l));
            if let Err(h) = r {
                self.raise(h);
                return;
            }
        }
    }

    /// The child's steps of registers it never read ([`Run::pic_output`]):
    /// the parent's own steps, unread where it never read the register
    /// (`State::add`), else added to its value.
    fn pic_take_steps(&mut self, steps: ValueId) {
        let v = self.v;
        for e in list_of(v, steps) {
            let [f, c, g] = list_of(v, e)[..] else {
                continue;
            };
            let loc = self.loc_of(Field(f));
            let c = i32::try_from(int_of(v, c)).unwrap_or(0);
            let global = int_of(v, g) != 0;
            self.muts += 1;
            if self.st.add(v, loc, c, global) {
                continue;
            }
            let x = match self.get_loc(loc) {
                Ok(x) => crate::val::i32_of(v, x),
                Err(h) => {
                    self.raise(h);
                    return;
                }
            };
            self.set_loc(loc, Entry::int(x.wrapping_add(c)), global);
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

    /// Whether the parent handed this child a binding of field `f`
    /// ([`Run::pic_locals`]), read as that key's presence alone: a
    /// register the parent defined or read is the parent's value, never a
    /// symbol of the register reaching the parent (`Run::defer_at`).
    pub(crate) fn pic_overlay_has(&mut self, f: Field) -> R<bool> {
        let view = self.pic_view()?;
        let v = self.v;
        Ok(self.cx.get(view, Step::Has(f.0)) == Some(v.bool(true)))
    }

    /// The view on the parent's bindings ([`Run::pic_locals`]).
    fn pic_view(&mut self) -> R<View> {
        let Some(p) = self.pic_child.as_mut() else {
            return Err(Halt::Starve);
        };
        if let Some(w) = p.view {
            return Ok(w);
        }
        let field = p.field;
        let w = self.cx.view_field(field)?;
        if let Some(p) = self.pic_child.as_mut() {
            p.view = Some(w);
        }
        Ok(w)
    }

    /// A picture child's base level, its own list empty, looked past its
    /// start (its last item, `\\unskip`, `\\lastbox`…): the parent's
    /// item before those it took already put in front of it, as it stands
    /// at the picture, so the look sees what TeX's picture saw. Read as
    /// that item alone (by the list's length and its place from the end):
    /// the parent's items before it may change and the child stands
    /// (Grail's figure: an added figure before it moved the parent's
    /// caption text, and the 1.5 s picture child that looked only at the
    /// last item ran again, accl 25873). Whether one was taken.
    pub(crate) fn pic_take_list(&mut self) -> R<bool> {
        let v = self.v;
        let Some(p) = self.pic_child.as_ref() else {
            return Ok(false);
        };
        let (lid, k) = (p.lid, p.took);
        let view = self.pic_view()?;
        let Some(pl) = self.cx.at(view, Step::Key(v.str(PARENT_LIST))) else {
            return Ok(false);
        };
        let Some(iv) = self.cx.at(pl, Step::Index(0)) else {
            return Ok(false);
        };
        let n = self
            .cx
            .len(iv)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0);
        if k >= n {
            return Ok(false);
        }
        let Some(item) = u32::try_from(n - 1 - k)
            .ok()
            .and_then(|i| self.cx.get(iv, Step::Index(i)))
        else {
            return Ok(false);
        };
        let since = self
            .cx
            .get(pl, Step::Index(1))
            .and_then(|x| usize::try_from(int_of(v, x)).ok())
            .unwrap_or(0);
        if let Some(p) = self.pic_child.as_mut() {
            p.took = k + 1;
        }
        self.muts += 1;
        if trace() {
            eprintln!(
                "pic: child {} looks before its list at {:?} cmd {} chr {}: takes the parent's item {} from its end of {n}",
                self.cx.node().raw(),
                self.cur.tok,
                self.cur.cmd,
                self.cur.chr,
                k + 1
            );
        }
        if let Some(l) = self.level_of(lid) {
            l.list.insert(0, Item::V(item));
            if k < since {
                l.fold.since_bp += 1;
            }
        }
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
        self.cx.read_before_placer_over(Field(a(1)));
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
            lid,
            took: 0,
            streams: Vec::new(),
            base_steps: Vec::new(),
        });
        // (the parent's appends to lists it never read, pending at the
        // picture: the child's to make or go on with, `Run::append_def`)
        let view = self.pic_view()?;
        let pending = self.cx.get(view, Step::Key(v.str(APPENDING)));
        for e in pending.map(|x| list_of(v, x)).unwrap_or_default() {
            if let [f, t] = list_of(v, e)[..] {
                let loc = self.loc_of(Field(f));
                self.appending.push((loc, toks_of(v, Some(t))));
            }
        }
        let base = self.cx.get(view, Step::Key(v.str(PARENT_STEPS)));
        for e in base.map(|x| list_of(v, x)).unwrap_or_default() {
            if let [f, c, g] = list_of(v, e)[..] {
                let loc = self.loc_of(Field(f));
                let c = i32::try_from(int_of(v, c)).unwrap_or(0);
                if self.st.add(v, loc, c, int_of(v, g) != 0)
                    && let Some(p) = self.pic_child.as_mut()
                {
                    p.base_steps.push((Field(f), c));
                }
            }
        }
        let pending = self.cx.get(view, Step::Key(v.str(APPENDING_VALS)));
        for e in pending.map(|x| list_of(v, x)).unwrap_or_default() {
            if let [f, xs] = list_of(v, e)[..] {
                let loc = self.loc_of(Field(f));
                self.appending_vals.push((loc, list_of(v, xs)));
            }
        }
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
            // (the lines read into bodies nothing looked at and nothing
            // after it sees, `\begin`'s `\@currenvline` closed by its
            // `\end`: taken back, as a piece's are; read, every picture
            // ran again for each line inserted above it)
            self.settle_lines();
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
        for (_, _, l) in self.publication() {
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
        // (its appends pending too: kept, they named locations of the
        // table dropped, which the empty one does not have, and their
        // publication asked the build's fields for a spill location's
        // field: `Fields::field_of`)
        self.appending.clear();
        self.appending_vals.clear();
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

    /// The child's steps of registers it never read, as its output
    /// carries them: `[field, c, global]` each.
    fn pic_steps(&mut self) -> Vec<ValueId> {
        let v = self.v;
        // (its steps of registers it never read: the parent's, made as
        // steps there, `State::add`)
        let base: Vec<(Field, i64)> = self
            .pic_child
            .as_ref()
            .map(|p| p.base_steps.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|(f, c)| (f, i64::from(c)))
            .collect();
        self.publication()
            .into_iter()
            .filter_map(|(f, w, l)| match w {
                crate::state::Pub::Rel(c) => {
                    // (the parent's steps it started from are the parent's)
                    let c = c - base.iter().find(|b| b.0 == f).map_or(0, |b| b.1);
                    (c != 0).then(|| {
                        v.list(vec![f.0, v.int(c), v.int(i64::from(l == Locality::Global))])
                    })
                }
                _ => None,
            })
            .collect()
    }

    /// The child's output: `[0, items, sf, pd, since_bp, lang, packed,
    /// clang, prevgraf, pushed level, locals, global names, read names,
    /// astate, aopen, ic_prev, aftergroup, contributions, streams, file
    /// reads, document lines begun, appends pending, values appended
    /// pending, whether its list holds the parent's before it, register
    /// steps]`; the run itself
    /// then contributes nothing.
    fn pic_output(&mut self) -> ValueId {
        let v = self.v;
        let took = self.pic_child.as_ref().map_or(0, |p| p.took);
        let steps = self.pic_steps();
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
        let appends: Vec<ValueId> = std::mem::take(&mut self.appending)
            .into_iter()
            .map(|(l, t)| v.list(vec![self.loc_field(l).0, toks_value(v, &t)]))
            .collect();
        let appends_vals: Vec<ValueId> = std::mem::take(&mut self.appending_vals)
            .into_iter()
            .map(|(l, xs)| v.list(vec![self.loc_field(l).0, v.list(xs)]))
            .collect();
        let mut locals = Vec::new();
        let mut gdefs = Vec::new();
        for (f, x, l) in self.published_values() {
            match l {
                Locality::Local => locals.push(v.list(vec![f.0, x])),
                _ => gdefs.push(v.list(vec![f.0, x])),
            }
        }
        // (the names read: the engine's record has them)
        let reads: Vec<ValueId> = Vec::new();
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
            v.list(appends),
            v.list(appends_vals),
            v.int(i64::try_from(took).unwrap_or(0)),
            v.list(steps),
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
    fn a_diagnostics_place_in_a_body_is_the_files_again() {
        let v = phitex_defuse::Values::new();
        let (doc, other) = (v.str("doc"), v.str("other"));
        let base = Some((doc, 100));
        let frame = |f, at| {
            v.list(vec![
                v.str("s"),
                v.str("x"),
                v.int(0),
                v.int(-2),
                v.int(1),
                f,
                v.int(at),
            ])
        };
        // (the body's lines from one past its start; another file's as they are)
        let at = |f| int_of(&v, list_of(&v, f)[6]);
        assert_eq!(at(rebase_frame(&v, frame(doc, 100), base)), 1);
        assert_eq!(at(rebase_frame(&v, frame(doc, 140), base)), 41);
        assert_eq!(at(rebase_frame(&v, frame(other, 140), base)), 140);
        let diag = |f, at| v.list(vec![v.int(0), v.str("m"), f, v.int(at), v.int(7)]);
        let ds = v.seq_concat(v.seq_unit(diag(doc, 41)), diag(other, 5));
        let back: Vec<i64> = v
            .seq_to_vec(rebase_diags(&v, ds, base))
            .into_iter()
            .map(|e| int_of(&v, list_of(&v, e)[3]))
            .collect();
        assert_eq!(back, [140, 5]);
    }

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
