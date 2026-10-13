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
/// [`Range`]; nothing is freed until the run ends, except the scratch a
/// macro call uses while matching its arguments ([`Arena::mark`],
/// [`Arena::release`]).
#[derive(Default)]
pub(crate) struct Arena {
    toks: Vec<Tok>,
}

impl Arena {
    pub(crate) fn with_capacity(n: usize) -> Arena {
        Arena {
            toks: Vec::with_capacity(n),
        }
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
        self.toks.extend_from_slice(xs);
        Range {
            start,
            end: self.mark(),
        }
    }

    /// Begins a list built token by token ([`Arena::push`]); its range is
    /// `Range { start, end: self.mark() }` when done.
    #[inline]
    pub(crate) fn begin(&self) -> u32 {
        self.mark()
    }

    #[inline]
    pub(crate) fn push(&mut self, t: Tok) {
        self.toks.push(t);
    }

    #[inline]
    pub(crate) fn extend(&mut self, xs: &[Tok]) {
        self.toks.extend_from_slice(xs);
    }

    #[inline]
    pub(crate) fn finish(&self, start: u32) -> Range {
        Range {
            start,
            end: self.mark(),
        }
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
    /// Where `buf[0]` is in a file: the file's name (a string value) and
    /// byte position, for diagnostics.
    pub origin: Option<(phitex_defuse::ValueId, usize)>,
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
    /// The node's text below the levels: segments in order, `text_at`
    /// the next to reach.
    pub text: Vec<Seg>,
    pub text_at: usize,
}

impl Input {
    pub(crate) fn new() -> Input {
        Input {
            arena: Arena::with_capacity(1 << 14),
            levels: Vec::with_capacity(64),
            lines: Vec::new(),
            free_lines: Vec::new(),
            files: Vec::new(),
            free_files: Vec::new(),
            text: Vec::new(),
            text_at: 0,
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
        while let Some(l) = self.levels.last() {
            if l.kind == kind::TOKS && l.a >= l.b {
                self.levels.pop();
            } else {
                break;
            }
        }
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
        let seg = std::mem::replace(&mut self.text[self.text_at], Seg::Toks(Vec::new()));
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
            if let (Some(Seg::Line(last)), Seg::Line(next)) = (self.text.last_mut(), &s)
                && !last.ends
                && !next.starts
            {
                last.buf.extend_from_slice(&next.buf);
                last.ends = next.ends;
                continue;
            }
            self.text.push(s);
        }
    }

    /// The last segment of the text (the one a continuation would join).
    pub(crate) fn text_last(&self) -> Option<&Seg> {
        self.text.last()
    }

    /// Where the levels stand, for a rewind: each level copied (four
    /// words), plus the lines' and files' positions.
    pub(crate) fn snapshot(&self, into: &mut Snapshot) {
        into.levels.clear();
        into.levels.extend_from_slice(&self.levels);
        into.lines.clear();
        for l in &self.levels {
            match l.kind {
                kind::LINE => {
                    let x = &self.lines[l.a as usize];
                    into.lines.push((x.loc, x.state, x.endch, x.buf.len()));
                }
                kind::FILE => {
                    let f = &self.files[l.a as usize];
                    into.lines.push((f.off, 0, 0, f.pos_chars()));
                }
                _ => {}
            }
        }
        into.text_at = self.text_at;
        into.arena = self.arena.mark();
    }

    /// Puts the levels back as [`Input::snapshot`] found them. False if
    /// a line was reduced in place beyond what can be restored.
    pub(crate) fn restore(&mut self, s: &Snapshot) -> bool {
        // levels pushed since are dropped (their lines/files freed)
        while self.levels.len() > s.levels.len() {
            self.pop();
        }
        self.levels.clear();
        self.levels.extend_from_slice(&s.levels);
        let mut k = 0;
        for l in &s.levels {
            match l.kind {
                kind::LINE => {
                    let (loc, state, endch, len) = s.lines[k];
                    k += 1;
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
                    let (off, _, _, pos) = s.lines[k];
                    k += 1;
                    let Some(f) = self.files.get_mut(l.a as usize) else {
                        return false;
                    };
                    f.off = off;
                    f.set_pos_chars(pos);
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

/// Where the input stood when a command began ([`Input::snapshot`]).
#[derive(Default)]
pub(crate) struct Snapshot {
    pub levels: Vec<Level>,
    /// `(loc, state, endch, len)` per line level, `(off, _, _, pos)` per
    /// file level, in level order.
    pub lines: Vec<(usize, u8, i64, usize)>,
    pub text_at: usize,
    pub arena: u32,
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
}
