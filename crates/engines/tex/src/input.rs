//! The input stack of one run (§300–§325) over flat arrays: every token
//! list a run reads lives in one arena, a level is four words, and the
//! fast path of `get_next` is one bounds test and one load.
//!
//! The stack has the expansion levels on top (token lists, the lines of
//! files `\input` inline, and the rest of such a file, as TeX's
//! `input_stack`) and, below them, the node's own text: its segments and
//! the siblings it claims, read in order ([`Input::text`]). A token list
//! read to its end is popped before another is pushed (§325, "conserve
//! stack space"), so the stack stays flat under tail recursion and a
//! level at the top always has a token left or is a line or a file.

use crate::val::Tok;

/// A range of the arena: the tokens `arena[start..end]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) struct Range {
    pub start: u32,
    pub end: u32,
}

impl Range {
    #[inline]
    pub(crate) fn is_empty(self) -> bool {
        self.end == self.start
    }
}

/// The run's token arena: a bump allocator of token lists. A list is a
/// [`Range`]; nothing is freed until the run ends, except the scratch
/// region a macro call matches its arguments into ([`Arena::scratch`]).
#[derive(Default)]
pub(crate) struct Arena {
    toks: Vec<Tok>,
    /// The scratch region (spec-final §2.3): a macro call's arguments,
    /// one after another, and each argument's bounds in it; emptied to
    /// the call's start when the call is done ([`Arena::scratch_end`]).
    scratch: Vec<Tok>,
    sbounds: Vec<(u32, u32)>,
    /// The macro bodies copied in (§390: read in place), by their shared
    /// list: a range is only good in the arena that holds it, so the
    /// cache lives and dies with it.
    bodies: phitex_defuse::FxHashMap<usize, (std::rc::Rc<[Tok]>, Range)>,
}

thread_local! {
    /// The token vectors of the thread's arenas dropped (a run's, or
    /// the one a compaction leaves), emptied: the next arena takes one
    /// as large as it asks for instead of growing a new one again.
    static ARENAS: std::cell::RefCell<Vec<Vec<Tok>>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The largest token vector [`ARENAS`] keeps (64 MB).
const ARENA_KEEP: usize = 1 << 24;

impl Drop for Arena {
    fn drop(&mut self) {
        let mut t = std::mem::take(&mut self.toks);
        if t.capacity() > 0 && t.capacity() <= ARENA_KEEP {
            t.clear();
            ARENAS.with(|p| {
                let mut p = p.borrow_mut();
                if p.len() < 2 {
                    p.push(t);
                }
            });
        }
    }
}

impl Arena {
    pub(crate) fn with_capacity(n: usize) -> Arena {
        let toks = ARENAS
            .with(|p| {
                let mut p = p.borrow_mut();
                let i = p.iter().position(|t| t.capacity() >= n)?;
                Some(p.swap_remove(i))
            })
            .unwrap_or_else(|| Vec::with_capacity(n));
        Arena {
            toks,
            scratch: Vec::with_capacity(256),
            sbounds: Vec::with_capacity(16),
            bodies: phitex_defuse::FxHashMap::default(),
        }
    }

    /// Tokens allocated so far.
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.toks.len()
    }

    /// The range of a macro body: copied in on its first call, found by
    /// its shared list after. Short bodies are copied every time (cheaper
    /// than the lookup).
    pub(crate) fn body(&mut self, body: &std::rc::Rc<[Tok]>) -> Range {
        if body.len() <= 8 {
            return self.alloc(body);
        }
        let key = std::rc::Rc::as_ptr(body).cast::<Tok>() as usize;
        if let Some((_, r)) = self.bodies.get(&key) {
            return *r;
        }
        let r = self.alloc(body);
        self.bodies.insert(key, (body.clone(), r));
        r
    }

    #[inline]
    pub(crate) fn get(&self, r: Range) -> &[Tok] {
        &self.toks[r.start as usize..r.end as usize]
    }

    #[inline]
    pub(crate) fn at(&self, i: u32) -> Tok {
        self.toks[i as usize]
    }

    #[inline]
    pub(crate) fn set(&mut self, i: u32, t: Tok) {
        self.toks[i as usize] = t;
    }

    /// The end of what is allocated: the start of the next list.
    #[inline]
    pub(crate) fn mark(&self) -> u32 {
        u32::try_from(self.toks.len()).unwrap_or(u32::MAX)
    }

    /// Copies `xs` in as one list.
    pub(crate) fn alloc(&mut self, xs: &[Tok]) -> Range {
        let start = self.mark();
        push_short(&mut self.toks, xs);
        Range {
            start,
            end: self.mark(),
        }
    }
}

/// Appends `xs` to `to`: a single token pushed, more copied.
#[inline]
fn push_short(to: &mut Vec<Tok>, xs: &[Tok]) {
    match xs {
        [] => {}
        [t] => to.push(*t),
        _ => to.extend_from_slice(xs),
    }
}

/// Where a macro call's part of the arena's scratch region begins
/// ([`Arena::scratch_begin`]): its tokens and its arguments' bounds.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Scratch {
    toks: u32,
    bounds: u32,
}

impl Arena {
    /// A macro call's scratch begins here.
    #[inline]
    pub(crate) fn scratch_begin(&self) -> Scratch {
        Scratch {
            toks: u32::try_from(self.scratch.len()).unwrap_or(u32::MAX),
            bounds: u32::try_from(self.sbounds.len()).unwrap_or(u32::MAX),
        }
    }

    /// The tokens matched so far since `b`.
    #[inline]
    pub(crate) fn scratch_len(&self, b: Scratch) -> usize {
        self.scratch.len() - b.toks as usize
    }

    #[inline]
    pub(crate) fn scratch_push(&mut self, t: Tok) {
        self.scratch.push(t);
    }

    /// The `i`-th token matched since `b`.
    #[inline]
    pub(crate) fn scratch_at(&self, b: Scratch, i: usize) -> Tok {
        self.scratch[b.toks as usize + i]
    }

    /// The next argument: tokens `lo..hi` since the call's start.
    #[inline]
    pub(crate) fn scratch_bound(&mut self, lo: usize, hi: usize) {
        self.sbounds.push((
            u32::try_from(lo).unwrap_or(u32::MAX),
            u32::try_from(hi).unwrap_or(u32::MAX),
        ));
    }

    /// §400: `body` with the call's arguments (since `b`) put in for its
    /// parameters, by its segments, written as one list.
    pub(crate) fn expand_body(
        &mut self,
        body: &[Tok],
        segs: &[crate::val::Seg],
        b: Scratch,
    ) -> Range {
        let start = self.mark();
        let (t0, b0) = (b.toks as usize, b.bounds as usize);
        // (the room made once for all the pieces)
        let arg = |sb: &[(u32, u32)], p: u8| {
            if p == 0 {
                return None;
            }
            sb.get(b0 + usize::from(p) - 1).map(|&(lo, hi)| t0 + lo as usize..t0 + hi as usize)
        };
        let n: usize = segs
            .iter()
            .map(|s| (s.hi - s.lo) as usize + arg(&self.sbounds, s.param).map_or(0, |r| r.len()))
            .sum();
        self.toks.reserve(n);
        for s in segs {
            push_short(&mut self.toks, &body[s.lo as usize..s.hi as usize]);
            if let Some(r) = arg(&self.sbounds, s.param) {
                push_short(&mut self.toks, &self.scratch[r]);
            }
        }
        Range {
            start,
            end: self.mark(),
        }
    }

    /// Argument `i` (0-based) of the call begun at `b`, as matched.
    pub(crate) fn scratch_arg(&self, b: Scratch, i: usize) -> Option<&[Tok]> {
        let &(lo, hi) = self.sbounds.get(b.bounds as usize + i)?;
        let t0 = b.toks as usize;
        self.scratch.get(t0 + lo as usize..t0 + hi as usize)
    }

    /// The call begun at `b` is done: its scratch goes.
    #[inline]
    pub(crate) fn scratch_end(&mut self, b: Scratch) {
        self.scratch.truncate(b.toks as usize);
        self.sbounds.truncate(b.bounds as usize);
    }
}

/// The kinds of a level (§307's `token_type`s, and a line or a file).
pub(crate) mod kind {
    /// A token list: `a` is the position, `b` the end (arena indices),
    /// `c` the start (kept for a rewind, §325 `backed_up` and the
    /// macro's name for diagnostics are the run's).
    pub const TOKS: u8 = 0;
    /// The line of a file being scanned: `a` is its index in
    /// [`super::Input::lines`].
    pub const LINE: u8 = 1;
    /// The rest of a file `\input` inline: `a` is its index in
    /// [`super::Input::files`].
    pub const FILE: u8 = 2;
}

/// One level of the input stack: four words, copied freely.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Level {
    pub kind: u8,
    /// For a token list, §307's token type (what pushed it): a parameter,
    /// a template, backed-up tokens, inserted tokens, a macro, an
    /// `\output` or `\every…` or mark or `\write` text. Diagnostics only.
    pub ty: u8,
    pub a: u32,
    pub b: u32,
    pub c: u32,
}

impl Level {
    #[inline]
    pub(crate) fn toks(r: Range, ty: u8) -> Level {
        Level {
            kind: kind::TOKS,
            ty,
            a: r.start,
            b: r.end,
            c: r.start,
        }
    }

    #[inline]
    pub(crate) fn line(i: usize) -> Level {
        Level {
            kind: kind::LINE,
            ty: 0,
            a: u32::try_from(i).unwrap_or(u32::MAX),
            b: 0,
            c: 0,
        }
    }

    #[inline]
    pub(crate) fn file(i: usize) -> Level {
        Level {
            kind: kind::FILE,
            ty: 0,
            a: u32::try_from(i).unwrap_or(u32::MAX),
            b: 0,
            c: 0,
        }
    }

    /// The tokens left of a token-list level.
    #[inline]
    pub(crate) fn rest(self) -> Range {
        Range {
            start: self.a,
            end: self.b,
        }
    }
}

/// §307 token types, for diagnostics and §325's rules (the ones in use).
pub(crate) mod ty {
    pub const BACKED_UP: u8 = 3;
    pub const MACRO: u8 = 5;
    /// A list no diagnostic names (`\the`, `\csname`, conversions).
    pub const OTHER: u8 = 17;
}

/// A line of a file or of the node's text (§300's `buffer`, `loc`,
/// `limit`, `state`): the characters, with `\endlinechar` appended once
/// its end is reached.
#[derive(Clone, Debug, Default)]
pub(crate) struct Line {
    pub buf: Vec<u32>,
    pub loc: usize,
    /// 0 `mid_line`, 1 `skip_blanks`, 2 `new_line` (§303).
    pub state: u8,
    /// -2 until the segment is reached (`\endlinechar` not yet applied),
    /// -3 reached, the end not yet; -1 the rest of the line was
    /// discarded; else the `\endlinechar` appended (or none, >= 256).
    pub endch: i64,
    /// Whether the segment begins a line (it is counted) and whether it
    /// ends one (`\endlinechar` goes after it).
    pub starts: bool,
    pub ends: bool,
    /// Where `buf[0]` is in a file: the file's name (a string value),
    /// byte position, and the written file's unit it is in (`wunit:`
    /// field's name, its position then from the unit's start), else
    /// `UNIT`; for diagnostics.
    pub origin: Option<(phitex_defuse::ValueId, usize, phitex_defuse::ValueId)>,
    /// Its line number in its file (`line`, §304), where the node knows
    /// it: a line of a file read here (its text at hand) or a pseudo
    /// file's; 0 for a document's piece, whose number is the lines of
    /// its file before the node ([`crate::run::Run::input_line_no`]).
    pub lineno: u32,
    /// Text read under the catcodes in force when it is read, which no
    /// analysis of a file saw: a pseudo file's lines (pdfTeX §1766) and
    /// a `\read` line (§483).
    pub rescanned: bool,
}

pub(crate) const MID_LINE: u8 = 0;
pub(crate) const SKIP_BLANKS: u8 = 1;
pub(crate) const NEW_LINE: u8 = 2;

impl Line {
    /// Whether characters are left, or the line's end is still to come.
    #[inline]
    pub(crate) fn has_more(&self) -> bool {
        self.loc < self.buf.len() || (self.ends && self.endch < -1)
    }

    /// A line from `text` (one line of a file, as web2c's `input_line`
    /// reads it: trailing blanks removed), its `\endlinechar` applied.
    pub(crate) fn of_str(line: &str, endlinechar: i64, rescanned: bool) -> Line {
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
        Line {
            buf,
            loc: 0,
            state: NEW_LINE,
            endch: endlinechar,
            starts: true,
            ends: true,
            origin: None,
            lineno: 0,
            rescanned,
        }
    }
}

/// A segment of the node's own text, or a sibling's, not yet reached
/// (decoded from the node's arguments when it is reached).
#[derive(Clone, Debug)]
pub(crate) enum Seg {
    Line(Line),
    Toks(Vec<Tok>),
    File(crate::run::FileRest),
}

impl Seg {
}

/// The input stack.
#[derive(Default)]
pub(crate) struct Input {
    pub arena: Arena,
    /// The levels, the top last.
    pub levels: Vec<Level>,
    /// The lines of [`kind::LINE`] levels, by index; a slot is reused
    /// once its level is popped ([`Input::free_lines`]).
    pub lines: Vec<Line>,
    free_lines: Vec<u32>,
    /// The file rests of [`kind::FILE`] levels.
    pub files: Vec<crate::run::FileRest>,
    free_files: Vec<u32>,
    /// Per line and file slot, the push that filled it last (a slot is
    /// reused once its level pops): a snapshot names its slots with
    /// these, so a restore after the slot took another line refuses
    /// instead of putting the old line's position on the new line.
    line_push: Vec<u64>,
    file_push: Vec<u64>,
    pushes: u64,
    /// The node's text below the levels: segments in order, `text_at`
    /// the next to reach.
    pub text: Vec<Seg>,
    pub text_at: usize,
    /// The arena's size, in tokens, past which the run compacts it
    /// between commands ([`Input::compact`]): four times what survived
    /// the last compaction, at least [`ARENA_COMPACT`].
    pub compact_at: usize,
    /// The levels as the command in progress found them, saved as they
    /// come on top ([`Input::snapshot`]).
    snap: Option<Snap>,
}

/// The levels a command found, saved lazily ([`Input::snapshot`]): a
/// level below the top is read only once it is on top again, so each is
/// saved when it comes on top (the top when the command began, then
/// each one a pop uncovers). The levels below `from` are as the command
/// found them; `saved[k]` is the one at index `len - 1 - k`.
struct Snap {
    len: usize,
    from: usize,
    saved: Vec<Saved>,
}

/// A level saved for a rewind ([`Snap`]): the level, its line's
/// `(loc, state, endch, len)` or its file's `(off, _, _, pos)`, and the
/// push that filled its slot.
type Saved = (Level, SavedPos, u64);

/// A saved line's `(loc, state, endch, len)` or file's `(off, _, _, pos)`.
type SavedPos = (usize, u8, i64, usize);

/// TeX's input stack's size (§30 `stack_size`, TeX Live's 10000).
pub(crate) const STACK_SIZE: usize = 10_000;

/// TeX's main memory (TeX Live's `main_memory`), in words.
pub(crate) const MAIN_MEMORY: usize = 5_000_000;

/// The arena's size, in tokens, past which one command's expansions
/// overflow the main memory: those it read are not freed before its
/// end ([`Input::compact`] runs between commands), and no command of a
/// document expands nearly so much.
pub(crate) const ARENA_MAX: usize = 1 << 27;

/// The least arena size, in tokens, at which a run compacts it.
pub(crate) const ARENA_COMPACT: usize = 1 << 22;

impl Input {
    pub(crate) fn new() -> Input {
        Input {
            arena: Arena::with_capacity(1 << 14),
            levels: Vec::with_capacity(64),
            lines: Vec::new(),
            free_lines: Vec::new(),
            files: Vec::new(),
            free_files: Vec::new(),
            line_push: Vec::new(),
            file_push: Vec::new(),
            pushes: 0,
            text: Vec::new(),
            text_at: 0,
            compact_at: ARENA_COMPACT,
            snap: None,
        }
    }

    /// Level `t` with its line's or file's position and the push that
    /// filled its slot, for [`Snap`].
    #[inline]
    fn saved(&self, t: usize) -> Saved {
        let l = self.levels[t];
        match l.kind {
            kind::LINE => {
                let x = &self.lines[l.a as usize];
                (
                    l,
                    (x.loc, x.state, x.endch, x.buf.len()),
                    self.line_push.get(l.a as usize).copied().unwrap_or(0),
                )
            }
            kind::FILE => {
                let f = &self.files[l.a as usize];
                (
                    l,
                    (f.off, 0, 0, f.pos_chars()),
                    self.file_push.get(l.a as usize).copied().unwrap_or(0),
                )
            }
            _ => (l, (0, 0, 0, 0), 0),
        }
    }

    /// After a pop: the level uncovered, saved for the command's rewind
    /// where it is one the command found and not saved yet.
    #[inline]
    fn uncovered(&mut self) {
        let Some(from) = self.snap.as_ref().map(|s| s.from) else {
            return;
        };
        let n = self.levels.len();
        if n > 0 && n - 1 < from {
            let e = self.saved(n - 1);
            if let Some(s) = self.snap.as_mut() {
                s.saved.push(e);
                s.from = n - 1;
            }
        }
    }

    #[inline]
    pub(crate) fn top(&self) -> Option<&Level> {
        self.levels.last()
    }

    #[inline]
    pub(crate) fn depth(&self) -> usize {
        self.levels.len()
    }

    /// Pushes a token list (§323 `begin_token_list`); an empty one is
    /// not pushed. Spent lists on top go first (§325).
    #[inline]
    pub(crate) fn push_toks(&mut self, r: Range, ty: u8) {
        if r.is_empty() {
            return;
        }
        self.pop_spent();
        self.levels.push(Level::toks(r, ty));
    }

    /// Pops the token-list levels on top that are read to their end.
    #[inline]
    pub(crate) fn pop_spent(&mut self) {
        if let Some(l) = self.levels.last()
            && l.kind == kind::TOKS
            && l.a >= l.b
        {
            self.pop_spent_slow();
        }
    }

    #[inline(never)]
    fn pop_spent_slow(&mut self) {
        while let Some(l) = self.levels.last() {
            if l.kind == kind::TOKS && l.a >= l.b {
                self.levels.pop();
                self.uncovered();
            } else {
                break;
            }
        }
    }

    /// Pops the level on top if it is a token list read to its end
    /// (§324); whether it did.
    #[inline]
    pub(crate) fn pop_spent_top(&mut self) -> bool {
        if let Some(l) = self.levels.last()
            && l.kind == kind::TOKS
            && l.a >= l.b
        {
            self.levels.pop();
            if self.snap.is_some() {
                self.uncovered();
            }
            return true;
        }
        false
    }

    /// Pushes a line on top.
    pub(crate) fn push_line(&mut self, line: Line) -> usize {
        let i = if let Some(i) = self.free_lines.pop() {
            self.lines[i as usize] = line;
            i as usize
        } else {
            self.lines.push(line);
            self.lines.len() - 1
        };
        self.pushes += 1;
        self.line_push.resize(self.lines.len(), 0);
        self.line_push[i] = self.pushes;
        self.levels.push(Level::line(i));
        i
    }

    /// Pushes a file's rest on top.
    pub(crate) fn push_file(&mut self, f: crate::run::FileRest) -> usize {
        let i = if let Some(i) = self.free_files.pop() {
            self.files[i as usize] = f;
            i as usize
        } else {
            self.files.push(f);
            self.files.len() - 1
        };
        self.pushes += 1;
        self.file_push.resize(self.files.len(), 0);
        self.file_push[i] = self.pushes;
        self.levels.push(Level::file(i));
        i
    }

    /// Pops the top level (§324 `end_token_list`, §329 `end_file_reading`).
    pub(crate) fn pop(&mut self) -> Option<Level> {
        let l = self.levels.pop()?;
        match l.kind {
            kind::LINE => self.free_lines.push(l.a),
            kind::FILE => self.free_files.push(l.a),
            _ => {}
        }
        self.uncovered();
        Some(l)
    }

    /// The line of a [`kind::LINE`] level.
    #[inline]
    pub(crate) fn line(&self, l: Level) -> &Line {
        &self.lines[l.a as usize]
    }

    #[inline]
    pub(crate) fn line_mut(&mut self, l: Level) -> &mut Line {
        &mut self.lines[l.a as usize]
    }

    /// The line on top, if the top level is one.
    #[inline]
    pub(crate) fn top_line_mut(&mut self) -> Option<&mut Line> {
        match self.levels.last() {
            Some(l) if l.kind == kind::LINE => Some(&mut self.lines[l.a as usize]),
            _ => None,
        }
    }

    #[inline]
    pub(crate) fn top_line(&self) -> Option<&Line> {
        match self.levels.last() {
            Some(l) if l.kind == kind::LINE => Some(&self.lines[l.a as usize]),
            _ => None,
        }
    }

    /// The file of a [`kind::FILE`] level.
    #[inline]
    pub(crate) fn file_mut(&mut self, l: Level) -> &mut crate::run::FileRest {
        &mut self.files[l.a as usize]
    }

    #[inline]
    pub(crate) fn file(&self, l: Level) -> &crate::run::FileRest {
        &self.files[l.a as usize]
    }

    /// The next token of the list on top, if the top level is a list
    /// with a token left (the fast path of `get_next`).
    #[inline]
    pub(crate) fn next_of_top(&mut self) -> Option<Tok> {
        let l = self.levels.last_mut()?;
        if l.kind == kind::TOKS && l.a < l.b {
            let t = self.arena.at(l.a);
            l.a += 1;
            Some(t)
        } else {
            None
        }
    }

    /// [`Input::next_of_top`] for a token that is not a frozen control
    /// sequence: a frozen one is left in place (the slow path handles it).
    #[inline]
    pub(crate) fn next_plain_of_top(&mut self) -> Option<Tok> {
        let l = self.levels.last_mut()?;
        if l.kind == kind::TOKS && l.a < l.b {
            let t = self.arena.at(l.a);
            if t.is_frozen_cs() {
                return None;
            }
            l.a += 1;
            Some(t)
        } else {
            None
        }
    }

    /// `back_input` (§325): the token goes on top as a backed-up list,
    /// or, when it is the token just read from the list on top, that
    /// list steps back (no allocation).
    #[inline]
    pub(crate) fn back(&mut self, t: Tok) {
        if let Some(l) = self.levels.last_mut()
            && l.kind == kind::TOKS
            && l.a > l.c
            && self.arena.at(l.a - 1) == t
        {
            l.a -= 1;
            return;
        }
        self.pop_spent();
        let r = self.arena.alloc(&[t]);
        self.levels.push(Level::toks(r, ty::BACKED_UP));
    }

    /// Reaches the next segment of the text: pushed as a level (a line,
    /// a list or a file). False when the text is exhausted.
    pub(crate) fn reach_text(&mut self) -> bool {
        if self.text_at >= self.text.len() {
            return false;
        }
        // (the segment stays in the text: a rewind after a halt inside
        // the reached line puts `text_at` back and reaches it again)
        let seg = self.text[self.text_at].clone();
        self.text_at += 1;
        match seg {
            Seg::Line(l) => {
                self.push_line(l);
            }
            Seg::Toks(t) => {
                let r = self.arena.alloc(&t);
                self.levels.push(Level::toks(r, ty::OTHER));
            }
            Seg::File(f) => {
                self.push_file(f);
            }
        }
        true
    }

    /// The segments of the text not reached yet.
    pub(crate) fn text_rest(&self) -> &[Seg] {
        &self.text[self.text_at.min(self.text.len())..]
    }

    /// Appends segments to the text; a line segment continuing the line
    /// of the last one joins it (a control sequence's name may run across
    /// nodes).
    pub(crate) fn append_text(&mut self, segs: Vec<Seg>) {
        for s in segs {
            // (a reached segment stays in the text as a copy; a
            // continuation of the line on top joins that line instead,
            // `Run::join_continuation`; so does one of a line reached
            // already that waits in the text as a level a parked run
            // had, its `\endlinechar` not applied as it did not end: a
            // segment of its own after it, as after the line on top)
            if self.text.len() > self.text_at
                && let (Some(Seg::Line(last)), Seg::Line(next)) = (self.text.last_mut(), &s)
                && !last.ends
                && last.endch == -2
                && !next.starts
            {
                last.buf.extend_from_slice(&next.buf);
                last.ends = next.ends;
                continue;
            }
            self.text.push(s);
        }
    }

    /// Where the levels stand, for a rewind: the top level saved now
    /// (with its line's or file's position), each level below it when a
    /// pop uncovers it ([`Snap`]): a command reads the levels below the
    /// top only once they are on top, so no copy of the whole stack.
    pub(crate) fn snapshot(&mut self, into: &mut Snapshot) {
        let len = self.levels.len();
        let top = len.checked_sub(1).map(|t| self.saved(t));
        // (the last snapshot's vector reused, in place)
        let sn = self.snap.get_or_insert_with(|| Snap {
            len: 0,
            from: 0,
            saved: Vec::new(),
        });
        sn.len = len;
        sn.from = len.saturating_sub(1);
        sn.saved.clear();
        sn.saved.extend(top);
        into.text_at = self.text_at;
        into.arena = self.arena.mark();
    }


    /// Copies what the token-list levels still hold (each from its
    /// origin, for a step back, §325) into a fresh arena and drops the
    /// rest: lists read to their end, finished macro calls, bodies cached
    /// for calls to come (copied in again on their next call). Nothing
    /// else holds a place in the arena between commands, so a long run
    /// (a format's preamble of a million commands) stays as large as what
    /// its input stack holds.
    pub(crate) fn compact(&mut self) {
        let live: usize = self
            .levels
            .iter()
            .filter(|l| l.kind == kind::TOKS && l.a < l.b)
            .map(|l| (l.b - l.c) as usize)
            .sum();
        let mut fresh = Arena::with_capacity((live * 2).max(1 << 14));
        for l in &mut self.levels {
            if l.kind != kind::TOKS {
                continue;
            }
            if l.a >= l.b {
                *l = Level::toks(Range { start: 0, end: 0 }, l.ty);
                continue;
            }
            let r = fresh.alloc(&self.arena.toks[l.c as usize..l.b as usize]);
            let read = l.a - l.c;
            l.c = r.start;
            l.a = r.start + read;
            l.b = r.end;
        }
        self.compact_at = (fresh.len() * 4).max(ARENA_COMPACT);
        self.arena = fresh;
    }

}

/// Where the input stood when a command began ([`Input::snapshot`]).
#[derive(Default)]
pub(crate) struct Snapshot {
    pub text_at: usize,
    pub arena: u32,
}

#[cfg(test)]
impl Input {
    /// Puts the levels back as [`Input::snapshot`] found them. False if
    /// a line was reduced in place beyond what can be restored.
    pub(crate) fn restore(&mut self, s: &Snapshot) -> bool {
        let Some(sn) = self.snap.take() else {
            return false;
        };
        // levels pushed since are dropped (their lines/files freed), and
        // those the command found above `from`, saved
        while self.levels.len() > sn.from {
            self.pop();
        }
        // (each line or file slot must still hold the line or file it
        // held then: one spent since and refilled holds another)
        for (l, _, push) in &sn.saved {
            let now = match l.kind {
                kind::LINE => self.line_push.get(l.a as usize),
                kind::FILE => self.file_push.get(l.a as usize),
                _ => continue,
            };
            if now.copied() != Some(*push) {
                return false;
            }
        }
        debug_assert_eq!(sn.from + sn.saved.len(), sn.len);
        for &(l, (loc, state, endch, len), _) in sn.saved.iter().rev() {
            self.levels.push(l);
            match l.kind {
                kind::LINE => {
                    let Some(x) = self.lines.get_mut(l.a as usize) else {
                        return false;
                    };
                    if x.buf.len() < len {
                        return false;
                    }
                    x.buf.truncate(len);
                    x.loc = loc;
                    x.state = state;
                    x.endch = endch;
                }
                kind::FILE => {
                    let Some(f) = self.files.get_mut(l.a as usize) else {
                        return false;
                    };
                    f.off = loc;
                    f.set_pos_chars(len);
                }
                _ => {}
            }
        }
        self.text_at = s.text_at;
        // (the arena is not truncated: lists allocated by the abandoned
        // command are unreachable and the run ends soon after a park)
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arena_lists_and_levels() {
        let mut inp = Input::new();
        let r = inp.arena.alloc(&[Tok::c(1), Tok::c(2), Tok::c(3)]);
        inp.push_toks(r, ty::MACRO);
        assert_eq!(inp.next_of_top(), Some(Tok::c(1)));
        assert_eq!(inp.next_of_top(), Some(Tok::c(2)));
        inp.back(Tok::c(2));
        assert_eq!(inp.depth(), 1);
        assert_eq!(inp.next_of_top(), Some(Tok::c(2)));
        assert_eq!(inp.next_of_top(), Some(Tok::c(3)));
        assert_eq!(inp.next_of_top(), None);
        // a spent list goes before another is pushed (§325)
        let r2 = inp.arena.alloc(&[Tok::c(9)]);
        inp.push_toks(r2, ty::MACRO);
        assert_eq!(inp.depth(), 1);
        assert_eq!(inp.next_of_top(), Some(Tok::c(9)));
        // a token backed up that is not the one just read allocates
        inp.back(Tok::c(7));
        assert_eq!(inp.depth(), 1);
        assert_eq!(inp.next_of_top(), Some(Tok::c(7)));
    }

    #[test]
    fn snapshot_restores_levels() {
        let mut inp = Input::new();
        let r = inp.arena.alloc(&[Tok::c(1), Tok::c(2)]);
        inp.push_toks(r, ty::MACRO);
        let mut s = Snapshot::default();
        inp.snapshot(&mut s);
        assert_eq!(inp.next_of_top(), Some(Tok::c(1)));
        let r2 = inp.arena.alloc(&[Tok::c(5)]);
        inp.push_toks(r2, ty::MACRO);
        assert!(inp.restore(&s));
        assert_eq!(inp.depth(), 1);
        assert_eq!(inp.next_of_top(), Some(Tok::c(1)));
    }

    /// A snapshot taken on a line that is then spent, its slot refilled
    /// by the next line, must not restore: the slot index no longer names
    /// the line it was taken on (the old line's position and state would
    /// land on the new line's text).
    #[test]
    fn snapshot_refuses_a_refilled_line_slot() {
        let mut inp = Input::new();
        let a = inp.push_line(Line::of_str("ab", 13, false));
        let top = *inp.top().unwrap();
        inp.line_mut(top).loc = 1;
        let mut s = Snapshot::default();
        inp.snapshot(&mut s);
        // the line read to its end and popped; the next line takes its slot
        inp.pop();
        let b = inp.push_line(Line::of_str("wxyz", 13, false));
        assert_eq!(a, b, "the spent line's slot is reused");
        assert!(!inp.restore(&s), "a refilled slot holds another line");

        // the slot still holding its line restores
        let mut inp = Input::new();
        inp.push_line(Line::of_str("ab", 13, false));
        inp.snapshot(&mut s);
        let top = *inp.top().unwrap();
        inp.line_mut(top).loc = 2;
        assert!(inp.restore(&s));
        assert_eq!(inp.top_line().map(|l| l.loc), Some(0));
    }

    /// A line's continuation (a sibling's first segment, cut mid-line)
    /// joins the last text segment only while that segment has not been
    /// reached: a line reached already, as not ending (no
    /// `\endlinechar` after it), and waiting in the text as a level a
    /// parked run had, gets its continuation as a segment of its own,
    /// which takes the `\endlinechar` when it is reached, as after the
    /// line on top. (Joined, the line's end took none: a resumed run lost
    /// the space at the end of a line its node boundary cut.)
    #[test]
    fn a_continuation_does_not_join_a_reached_line() {
        let cut = |text: &str, starts: bool, ends: bool, endch: i64| {
            let mut l = Line::of_str(text, -1, false);
            l.starts = starts;
            l.ends = ends;
            l.endch = endch;
            l
        };
        // (reached: the parked run's line level, read to its cut)
        let mut inp = Input::new();
        let mut reached = cut("\\x{ab}", true, false, -1);
        reached.loc = reached.buf.len();
        inp.text.push(Seg::Line(reached));
        inp.append_text(vec![Seg::Line(cut(", and integral", false, true, -2))]);
        assert_eq!(inp.text.len(), 2, "a segment of its own");
        // (not reached: one line, its end still to come)
        let mut inp = Input::new();
        inp.text.push(Seg::Line(cut("\\x{ab}", true, false, -2)));
        inp.append_text(vec![Seg::Line(cut(", and integral", false, true, -2))]);
        assert_eq!(inp.text.len(), 1, "joined");
        let Some(Seg::Line(l)) = inp.text.first() else {
            panic!("a line")
        };
        assert!(l.ends);
    }
}
