//! `PhiTeX`'s concrete syntax tree: lossless, cut into paragraphs, and
//! reparsed incrementally.
//!
//! The tree is green and red, as in rust-analyzer: a green node knows its
//! kind, its length and its children, never its position, so a paragraph's
//! tree is kept as it is by every edit elsewhere; a red node ([`Red`]) is a
//! green node and the offset it is at, computed while walking down. Adding
//! a line changes lengths on one path, not every position after it.
//!
//! A token is only a kind and a length, inline in its parent: its text is
//! the paragraph's, which the paragraph keeps once ([`Para::text`]). So a
//! paragraph costs an allocation for itself and one per group, not per
//! token.
//!
//! The document's children are paragraphs: runs of lines up to and
//! including the blank lines that end them, at brace depth 0. Each has an
//! identity ([`ParaId`]) that lives as long as its text is not edited.
//! [`Tree::edit`] reparses the paragraphs an edit touches, and on until the
//! old paragraph boundaries come back; the others are kept as they were.
//!
//! The tree reads with plain TeX's catcodes. The document may change them;
//! the SSA builder reads the characters again with the real ones. For it a
//! paragraph boundary is only a place where it may stop, if its state is
//! clean there.
//!
//! One exception: LaTeX's verbatim constructs, whose text is characters,
//! not commands, are read as such ([`SyntaxKind::Verbatim`]): `\verb|…|`
//! (and `\lstinline`, `\mintinline`, `\Verb`, `\url`, `\nolinkurl`, the
//! URL of `\href`), and the body of the environments in
//! [`VERBATIM_ENVIRONMENTS`] up to their `\end{name}`. A brace or a `%` in
//! them neither opens a group nor starts a comment, and a blank line in a
//! verbatim body does not end the paragraph.

use std::ops::Range;
use std::rc::Rc;

/// What a green node or token is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SyntaxKind {
    Para,
    Group,
    /// `\name` or `\x`.
    Cs,
    /// Letters and other characters.
    Text,
    /// Spaces and tabs.
    Space,
    Newline,
    /// `%` up to (not including) the end of the line.
    Comment,
    /// `#` and its digit.
    Param,
    /// A group's `{` or `}`.
    Brace,
    /// A `}` with no group open.
    Other,
    /// Characters a LaTeX verbatim construct reads as they are: what
    /// follows `\verb` up to its closing delimiter (the star and the
    /// delimiters included), or a verbatim environment's body.
    Verbatim,
}

/// A green node: a kind, a length, and children; or a token, a kind and
/// a length.
#[derive(Debug, PartialEq, Eq)]
pub enum Green {
    Node {
        kind: SyntaxKind,
        len: usize,
        children: Vec<Green>,
    },
    Token {
        kind: SyntaxKind,
        len: usize,
    },
}

impl Green {
    #[must_use]
    pub fn kind(&self) -> SyntaxKind {
        match self {
            Green::Node { kind, .. } | Green::Token { kind, .. } => *kind,
        }
    }

    /// Its length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Green::Node { len, .. } | Green::Token { len, .. } => *len,
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[must_use]
    pub fn children(&self) -> &[Green] {
        match self {
            Green::Node { children, .. } => children,
            Green::Token { .. } => &[],
        }
    }
}

fn node(kind: SyntaxKind, children: Vec<Green>) -> Green {
    let len = children.iter().map(Green::len).sum();
    Green::Node {
        kind,
        len,
        children,
    }
}

/// A green node where it is: the red tree, made while walking down.
#[derive(Clone, Copy, Debug)]
pub struct Red<'a> {
    pub green: &'a Green,
    pub offset: usize,
}

impl<'a> Red<'a> {
    /// Its children, each at its offset.
    pub fn children(self) -> impl Iterator<Item = Red<'a>> {
        let mut at = self.offset;
        self.green.children().iter().map(move |g| {
            let r = Red {
                green: g,
                offset: at,
            };
            at += g.len();
            r
        })
    }

    #[must_use]
    pub fn end(self) -> usize {
        self.offset + self.green.len()
    }
}

/// A paragraph's identity: the same until its text is edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ParaId(pub u64);

/// A paragraph: its identity, its text and its green node.
#[derive(Clone, Debug)]
pub struct Para {
    pub id: ParaId,
    text: Rc<str>,
    pub green: Rc<Green>,
}

impl Para {
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// What an edit did to the paragraphs: `removed` of them, from index `at`,
/// were replaced by `inserted` new ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Splice {
    pub at: usize,
    pub removed: usize,
    pub inserted: usize,
}

/// The document: its paragraphs, in order.
#[derive(Clone, Debug, Default)]
pub struct Tree {
    paras: Vec<Para>,
    /// Where each paragraph ends.
    ends: Vec<usize>,
    next: u64,
}

impl Tree {
    #[must_use]
    pub fn parse(src: &str) -> Tree {
        let mut t = Tree::default();
        let mut pos = 0;
        while pos < src.len() {
            let e = para_end(src.as_bytes(), pos);
            let p = t.para(&src[pos..e]);
            t.paras.push(p);
            t.ends.push(e);
            pos = e;
        }
        t
    }

    /// Replace the paragraphs `at..at + removed` with `pieces`, each a
    /// paragraph of its own whatever its text (a file the build makes, cut
    /// where it was made: no reparse). The bytes it replaced, and how long
    /// the new ones are.
    ///
    /// # Panics
    ///
    /// If the paragraphs are not in the tree.
    pub fn replace_paras(
        &mut self,
        at: usize,
        removed: usize,
        pieces: &[&str],
    ) -> (Range<usize>, usize) {
        let begin = if at == 0 { 0 } else { self.ends[at - 1] };
        let old_end = if removed == 0 {
            begin
        } else {
            self.ends[at + removed - 1]
        };
        let mut new_ends = Vec::with_capacity(pieces.len());
        let mut e = begin;
        let paras: Vec<Para> = pieces
            .iter()
            .map(|p| {
                e += p.len();
                new_ends.push(e);
                self.para(p)
            })
            .collect();
        let new_len = e - begin;
        self.paras.splice(at..at + removed, paras);
        self.ends.splice(at..at + removed, new_ends);
        for x in &mut self.ends[at + pieces.len()..] {
            *x = *x - (old_end - begin) + new_len;
        }
        (begin..old_end, new_len)
    }

    fn para(&mut self, text: &str) -> Para {
        let id = ParaId(self.next);
        self.next += 1;
        Para {
            id,
            text: text.into(),
            green: Rc::new(lex(text)),
        }
    }

    #[must_use]
    pub fn paras(&self) -> &[Para] {
        &self.paras
    }

    /// The document's length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ends.last().copied().unwrap_or(0)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.paras.is_empty()
    }

    /// The document's text (the tree is lossless: it is the source).
    #[must_use]
    pub fn text(&self) -> String {
        let mut s = String::with_capacity(self.len());
        for p in &self.paras {
            s.push_str(&p.text);
        }
        s
    }

    /// Where each paragraph ends.
    #[must_use]
    pub fn para_ends(&self) -> &[usize] {
        &self.ends
    }

    /// The paragraphs as red nodes.
    pub fn reds(&self) -> impl Iterator<Item = Red<'_>> {
        let mut at = 0;
        self.paras.iter().map(move |p| {
            let r = Red {
                green: &p.green,
                offset: at,
            };
            at += p.green.len();
            r
        })
    }

    /// Replace `old_len` bytes at `start` with `text`: reparse from the
    /// paragraph before the edit until a paragraph ends where an old one
    /// after the edit ended; keep the rest. Only the paragraphs read are
    /// copied: the tree is the document's rope.
    ///
    /// # Panics
    ///
    /// If the bytes are not within the document, on character boundaries.
    pub fn edit(&mut self, start: usize, old_len: usize, text: &str) -> Splice {
        let n = self.ends.len();
        let old_end = start + old_len;
        let new_len = text.len();
        // (the one before too: a blank line added at the edit can move
        // where it ends)
        let at = self.ends.partition_point(|&e| e <= start).saturating_sub(1);
        let begin = if at == 0 { 0 } else { self.ends[at - 1] };
        let mut k = self.ends.partition_point(|&e| e <= old_end);
        // the new text from `begin`: the old paragraphs, edited, and as
        // many more as the reparse reads
        let mut buf = String::new();
        let mut next = (k + 1).min(n);
        for p in &self.paras[at..next] {
            buf.push_str(&p.text);
        }
        buf.replace_range(start - begin..old_end - begin, text);
        let mut new_ends = Vec::new();
        let mut pos = 0;
        loop {
            if pos >= buf.len() && next >= n {
                k = n;
                break;
            }
            let e = para_end(buf.as_bytes(), pos);
            if e == buf.len() && next < n {
                // (it may go on in the next paragraph)
                buf.push_str(&self.paras[next].text);
                next += 1;
                continue;
            }
            new_ends.push(begin + e);
            pos = e;
            let abs = begin + pos;
            while k < n && self.ends[k] - old_len + new_len < abs {
                k += 1;
            }
            if k < n && self.ends[k] - old_len + new_len == abs {
                k += 1;
                break;
            }
        }
        let mut from = 0;
        let mut paras = Vec::with_capacity(new_ends.len());
        for e in &new_ends {
            let e = e - begin;
            paras.push(self.para(&buf[from..e]));
            from = e;
        }
        let inserted = paras.len();
        self.paras.splice(at..k, paras);
        self.ends.splice(at..k, new_ends);
        for e in &mut self.ends[at + inserted..] {
            *e = *e - old_len + new_len;
        }
        Splice {
            at,
            removed: k - at,
            inserted,
        }
    }

    /// A reader of the document's bytes, by position.
    #[must_use]
    pub fn cursor(&self) -> Cursor<'_> {
        Cursor {
            paras: &self.paras,
            ends: &self.ends,
            start: 0,
            bytes: &[],
        }
    }
}

/// Reads the document's bytes by position, across paragraphs; reading
/// forward within a paragraph is an index.
#[derive(Clone, Copy)]
pub struct Cursor<'a> {
    paras: &'a [Para],
    ends: &'a [usize],
    /// The paragraph at hand: where it begins, and its bytes.
    start: usize,
    bytes: &'a [u8],
}

impl<'a> Cursor<'a> {
    /// The byte at `pos` (`None`: past the end).
    #[inline]
    pub fn byte(&mut self, pos: usize) -> Option<u8> {
        match pos.checked_sub(self.start) {
            Some(i) if i < self.bytes.len() => Some(self.bytes[i]),
            _ => {
                self.seek(pos)?;
                Some(self.bytes[pos - self.start])
            }
        }
    }

    /// Make the paragraph with `pos` the one at hand.
    fn seek(&mut self, pos: usize) -> Option<()> {
        let i = self.ends.partition_point(|&e| e <= pos);
        let p = self.paras.get(i)?;
        self.start = if i == 0 { 0 } else { self.ends[i - 1] };
        self.bytes = p.text.as_bytes();
        Some(())
    }

    /// The bytes `a..b`: borrowed if they are in one paragraph.
    pub fn bytes(&mut self, a: usize, b: usize) -> std::borrow::Cow<'a, [u8]> {
        if a < b && self.byte(a).is_some() && b - self.start <= self.bytes.len() {
            return std::borrow::Cow::Borrowed(&self.bytes[a - self.start..b - self.start]);
        }
        std::borrow::Cow::Owned((a..b).filter_map(|i| self.byte(i)).collect())
    }
}

fn blank(line: &[u8]) -> bool {
    line.iter().all(|&c| matches!(c, b' ' | b'\t' | b'\r'))
}

fn line_end(s: &[u8], from: usize) -> usize {
    s[from..]
        .iter()
        .position(|&c| c == b'\n')
        .map_or(s.len(), |i| from + i)
}

/// Where the paragraph that begins at `start` ends: after the blank lines
/// that follow its text at brace depth 0 (the blank lines before any text
/// are its own). A verbatim run is skipped whole: its braces, `%` and blank
/// lines are characters.
fn para_end(s: &[u8], start: usize) -> usize {
    let mut i = start;
    let mut depth = 0usize;
    let mut text = false;
    loop {
        if i >= s.len() {
            return s.len();
        }
        let mut e = line_end(s, i);
        if blank(&s[i..e]) {
            if depth == 0 && text {
                let mut j = (e + 1).min(s.len());
                while j < s.len() {
                    let e2 = line_end(s, j);
                    if !blank(&s[j..e2]) {
                        break;
                    }
                    j = (e2 + 1).min(s.len());
                }
                return j;
            }
        } else {
            text = true;
            let mut c = i;
            while c < e {
                match s[c] {
                    b'\\' => {
                        if let Some(v) = verbatim_at(s, c) {
                            // (to the line where the run ends)
                            c = v.end;
                            if c > e {
                                e = line_end(s, c);
                            }
                            continue;
                        }
                        c += 1;
                    }
                    b'%' => break,
                    b'{' => depth += 1,
                    b'}' => depth = depth.saturating_sub(1),
                    _ => {}
                }
                c += 1;
            }
        }
        if e >= s.len() {
            return s.len();
        }
        i = e + 1;
    }
}

/// The environments whose body LaTeX reads verbatim, up to the first
/// `\end{name}` (the kernel's, `verbatim.sty`'s, `comment.sty`'s, `fancyvrb`'s,
/// `listings`'s, `minted`'s and `filecontents`).
pub const VERBATIM_ENVIRONMENTS: &[&str] = &[
    "verbatim",
    "verbatim*",
    "Verbatim",
    "Verbatim*",
    "BVerbatim",
    "LVerbatim",
    "lstlisting",
    "minted",
    "comment",
    "filecontents",
    "filecontents*",
    "VerbatimOut",
    "SaveVerbatim",
];

/// The commands whose argument LaTeX reads verbatim.
pub const VERBATIM_COMMANDS: &[&str] = &[
    "verb",
    "Verb",
    "lstinline",
    "mintinline",
    "url",
    "nolinkurl",
    "href",
];

/// What follows `\begin{name}` of a verbatim environment before its body:
/// whether an optional argument may, and how many mandatory ones do.
fn verbatim_env_args(name: &[u8]) -> Option<(bool, usize)> {
    Some(match name {
        b"verbatim" | b"verbatim*" | b"comment" => (false, 0),
        b"Verbatim" | b"Verbatim*" | b"BVerbatim" | b"LVerbatim" | b"lstlisting" => (true, 0),
        b"minted" | b"filecontents" | b"filecontents*" | b"SaveVerbatim" => (true, 1),
        b"VerbatimOut" => (false, 1),
        _ => return None,
    })
}

/// How a verbatim command's text is delimited.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Delim {
    /// By any character, up to its next occurrence on the line (`\verb`).
    Char,
    /// By a character, or by balanced braces.
    CharOrBraces,
    /// By balanced braces only (`\href`'s URL).
    Braces,
}

/// A verbatim command: whether an optional argument may come first, how
/// many brace groups come first (`\mintinline`'s language), and its
/// delimiters.
fn verbatim_cmd(name: &[u8]) -> Option<(bool, usize, Delim)> {
    Some(match name {
        b"verb" => (false, 0, Delim::Char),
        b"Verb" => (true, 0, Delim::Char),
        b"lstinline" => (true, 0, Delim::CharOrBraces),
        b"mintinline" => (true, 1, Delim::CharOrBraces),
        b"url" | b"nolinkurl" => (false, 0, Delim::CharOrBraces),
        b"href" => (false, 0, Delim::Braces),
        _ => return None,
    })
}

/// A verbatim run: the bytes `start..end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Verb {
    start: usize,
    end: usize,
}

/// The verbatim run the command at `i` (a backslash) reads, if it is one of
/// LaTeX's verbatim constructs: for a command, everything after its name
/// up to the closing delimiter (the end of the line if there is none,
/// where LaTeX stops with an error); for `\begin{name}` of a verbatim
/// environment, the body after its arguments, up to `\end{name}` (or the
/// end of the text).
fn verbatim_at(s: &[u8], i: usize) -> Option<Verb> {
    let n0 = i + 1;
    let mut n1 = n0;
    while n1 < s.len() && s[n1].is_ascii_alphabetic() {
        n1 += 1;
    }
    let name = &s[n0..n1];
    if name == b"begin" {
        return verbatim_env(s, n1);
    }
    let (opt, groups, delim) = verbatim_cmd(name)?;
    let mut j = n1;
    if name == b"verb" && s.get(j) == Some(&b'*') {
        j += 1;
    }
    if delim != Delim::Char {
        j = skip_blanks(s, j);
    }
    if opt && s.get(j) == Some(&b'[') {
        j = bracket_end(s, j)?;
    }
    for _ in 0..groups {
        j = skip_blanks(s, j);
        if s.get(j) != Some(&b'{') {
            return None;
        }
        j = brace_end(s, j);
    }
    let d = *s.get(j)?;
    let end = match (delim, d) {
        (Delim::Braces | Delim::CharOrBraces, b'{') => brace_end(s, j),
        // (an ASCII delimiter only: a run never ends inside a character)
        (Delim::Char | Delim::CharOrBraces, d) if d.is_ascii() && d != b'\n' && d != b'\r' => {
            let le = line_end(s, j + 1);
            s[j + 1..le]
                .iter()
                .position(|&c| c == d)
                .map_or(le, |k| j + 1 + k + 1)
        }
        _ => return None,
    };
    Some(Verb { start: n1, end })
}

/// `\begin{name}` from `j` (after `\begin`): the body of a verbatim
/// environment.
fn verbatim_env(s: &[u8], j: usize) -> Option<Verb> {
    let j = skip_blanks(s, j);
    if s.get(j) != Some(&b'{') {
        return None;
    }
    // (an environment's name is short)
    let close = j + 1 + s[j + 1..].iter().take(32).position(|&c| c == b'}')?;
    let name = &s[j + 1..close];
    let (opt, groups) = verbatim_env_args(name)?;
    let mut k = close + 1;
    if opt {
        let t = skip_blanks(s, k);
        if s.get(t) == Some(&b'[') {
            k = bracket_end(s, t)?;
        }
    }
    for _ in 0..groups {
        let t = skip_blanks(s, k);
        if s.get(t) != Some(&b'{') {
            return None;
        }
        k = brace_end(s, t);
    }
    Some(Verb {
        start: k,
        end: find_end(s, k, name),
    })
}

/// Past the spaces and tabs from `j`.
fn skip_blanks(s: &[u8], mut j: usize) -> usize {
    while j < s.len() && matches!(s[j], b' ' | b'\t') {
        j += 1;
    }
    j
}

/// Past the `]` that closes the `[` at `j`, outside braces (`None`: there
/// is none, and LaTeX would not read an optional argument either).
fn bracket_end(s: &[u8], j: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (k, &c) in s.iter().enumerate().skip(j + 1) {
        match c {
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            b']' if depth == 0 => return Some(k + 1),
            _ => {}
        }
    }
    None
}

/// Past the `}` that closes the `{` at `j` (the end of the text if none
/// does).
fn brace_end(s: &[u8], j: usize) -> usize {
    let mut depth = 0usize;
    for (k, &c) in s.iter().enumerate().skip(j) {
        match c {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return k + 1;
                }
            }
            _ => {}
        }
    }
    s.len()
}

/// Where `\end{name}` is, from `from` (the end of the text if nowhere).
fn find_end(s: &[u8], from: usize, name: &[u8]) -> usize {
    let mut i = from;
    while let Some(k) = s[i..].iter().position(|&c| c == b'\\') {
        let p = i + k;
        let rest = &s[p + 1..];
        if rest.starts_with(b"end{")
            && rest[4..].starts_with(name)
            && rest.get(4 + name.len()) == Some(&b'}')
        {
            return p;
        }
        i = p + 1;
    }
    s.len()
}

/// The length of the UTF-8 character that begins with `b`.
fn utf8_len(b: u8) -> usize {
    match b {
        0..0xc0 => 1,
        0xc0..0xe0 => 2,
        0xe0..0xf0 => 3,
        _ => 4,
    }
}

/// A text as a green node, read with plain TeX's catcodes (and LaTeX's
/// verbatim constructs as [`SyntaxKind::Verbatim`] runs): a paragraph's,
/// or any other (a macro's body with its arguments put in).
#[must_use]
pub fn lex(text: &str) -> Green {
    let b = text.as_bytes();
    // (the children of the group being read, and of those around it)
    let mut cur: Vec<Green> = Vec::new();
    let mut outer: Vec<Vec<Green>> = Vec::new();
    let mut i = 0;
    // (a verbatim run ahead: the tokens before it stop where it begins)
    let mut verb: Option<Verb> = None;
    while i < b.len() {
        if let Some(v) = verb.filter(|v| v.start <= i) {
            verb = None;
            if v.end > i {
                cur.push(Green::Token {
                    kind: SyntaxKind::Verbatim,
                    len: v.end - i,
                });
                i = v.end;
            }
            continue;
        }
        match b[i] {
            b'{' => {
                outer.push(std::mem::take(&mut cur));
                cur.push(Green::Token {
                    kind: SyntaxKind::Brace,
                    len: 1,
                });
                i += 1;
            }
            b'}' if !outer.is_empty() => {
                cur.push(Green::Token {
                    kind: SyntaxKind::Brace,
                    len: 1,
                });
                let group = node(SyntaxKind::Group, std::mem::take(&mut cur));
                cur = outer.pop().unwrap_or_default();
                cur.push(group);
                i += 1;
            }
            c => {
                if c == b'\\' && verb.is_none() {
                    verb = verbatim_at(b, i);
                }
                let limit = verb.map_or(b.len(), |v| v.start);
                let (kind, end) = scan(b, i, limit);
                cur.push(Green::Token { kind, len: end - i });
                i = end;
            }
        }
    }
    // (a group still open at the end: closed here, without its brace)
    while let Some(mut parent) = outer.pop() {
        parent.push(node(SyntaxKind::Group, cur));
        cur = parent;
    }
    node(SyntaxKind::Para, cur)
}

/// The token at `i` (not a group's brace), read no further than `limit`:
/// its kind and where it ends.
fn scan(b: &[u8], mut i: usize, limit: usize) -> (SyntaxKind, usize) {
    let kind = match b[i] {
        b'\\' => {
            i += 1;
            if i < limit {
                if b[i].is_ascii_alphabetic() {
                    while i < limit && b[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                } else {
                    i = (i + utf8_len(b[i])).min(limit);
                }
            }
            SyntaxKind::Cs
        }
        b'}' => {
            i += 1;
            SyntaxKind::Other
        }
        b'%' => {
            while i < limit && b[i] != b'\n' {
                i += 1;
            }
            SyntaxKind::Comment
        }
        b'\n' => {
            i += 1;
            SyntaxKind::Newline
        }
        b' ' | b'\t' | b'\r' => {
            while i < limit && matches!(b[i], b' ' | b'\t' | b'\r') {
                i += 1;
            }
            SyntaxKind::Space
        }
        b'#' => {
            i += 1;
            if i < limit && b[i].is_ascii_digit() {
                i += 1;
            }
            SyntaxKind::Param
        }
        _ => {
            while i < limit
                && !matches!(
                    b[i],
                    b'\\' | b'{' | b'}' | b'%' | b'\n' | b' ' | b'\t' | b'\r' | b'#'
                )
            {
                i += 1;
            }
            SyntaxKind::Text
        }
    };
    (kind, i)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "\\def\\a{x}\n\n{\\b\n\nc}% hi\n  \n\ntext #1 }\n\n\n";

    #[test]
    fn lossless() {
        let t = Tree::parse(DOC);
        assert_eq!(t.text(), DOC);
        assert_eq!(t.len(), DOC.len());
    }

    /// Paragraphs end after their blank lines, never inside a group.
    #[test]
    fn paragraphs() {
        let t = Tree::parse(DOC);
        let texts: Vec<String> = t.paras().iter().map(|p| p.text().to_string()).collect();
        assert_eq!(
            texts,
            [
                "\\def\\a{x}\n\n",
                "{\\b\n\nc}% hi\n  \n\n",
                "text #1 }\n\n\n"
            ]
        );
    }

    #[test]
    fn red_offsets() {
        let t = Tree::parse(DOC);
        let mut last = 0;
        for r in t.reds() {
            assert_eq!(r.offset, last);
            let mut at = r.offset;
            for c in r.children() {
                assert_eq!(c.offset, at);
                at = c.end();
            }
            assert_eq!(at, r.end());
            last = r.end();
        }
        assert_eq!(last, DOC.len());
    }

    fn check_edit(src: &str, start: usize, old_len: usize, text: &str) -> Splice {
        let mut t = Tree::parse(src);
        let before: Vec<ParaId> = t.paras().iter().map(|p| p.id).collect();
        let mut new = src.to_string();
        new.replace_range(start..start + old_len, text);
        let s = t.edit(start, old_len, text);
        assert_eq!(t.text(), new);
        let fresh = Tree::parse(&new);
        assert_eq!(t.para_ends(), fresh.para_ends());
        // (the paragraphs outside the splice kept their identity)
        let after: Vec<ParaId> = t.paras().iter().map(|p| p.id).collect();
        assert_eq!(after[..s.at], before[..s.at]);
        assert_eq!(after[s.at + s.inserted..], before[s.at + s.removed..]);
        s
    }

    #[test]
    fn cursor() {
        let t = Tree::parse(DOC);
        let mut c = t.cursor();
        let read: Vec<u8> = (0..DOC.len()).filter_map(|i| c.byte(i)).collect();
        assert_eq!(read, DOC.as_bytes());
        assert_eq!(c.byte(DOC.len()), None);
        assert_eq!(&*c.bytes(1, 4), b"def");
        // (across a paragraph boundary)
        assert_eq!(&*c.bytes(10, 14), &DOC.as_bytes()[10..14]);
        assert_eq!(c.byte(3), Some(b'f'));
    }

    #[test]
    fn edits() {
        let src = "a\n\nb\n\nc\n\nd\n\ne\n";
        // a word inside `c`: `b` (the one before) and `c` are reparsed
        let s = check_edit(src, 6, 0, "cc");
        assert_eq!((s.at, s.removed, s.inserted), (1, 2, 2));
        // the blank line after `b` deleted: `b` and `c` become one
        let s = check_edit(src, 4, 1, "");
        assert_eq!(s.inserted + 1, s.removed);
        // a `{` opened in `b`: the rest is one paragraph
        check_edit(src, 3, 0, "{");
        // a new paragraph
        check_edit(src, 3, 0, "\n\nnew");
        // at the ends
        check_edit(src, 0, 0, "x\n\n");
        check_edit(src, src.len(), 0, "\nf\n");
        check_edit("", 0, 0, "only\n");
        check_edit(src, 0, src.len(), "");
    }

    fn texts(t: &Tree) -> Vec<&str> {
        t.paras().iter().map(Para::text).collect()
    }

    /// The tokens of a text, depth first, with their text (groups by
    /// their braces).
    fn tokens(text: &str) -> Vec<(SyntaxKind, &str)> {
        fn walk<'a>(
            g: &Green,
            text: &'a str,
            at: &mut usize,
            out: &mut Vec<(SyntaxKind, &'a str)>,
        ) {
            match g {
                Green::Token { kind, len } => {
                    out.push((*kind, &text[*at..*at + len]));
                    *at += len;
                }
                Green::Node { children, .. } => {
                    for c in children {
                        walk(c, text, at, out);
                    }
                }
            }
        }
        let mut out = Vec::new();
        walk(&lex(text), text, &mut 0, &mut out);
        out
    }

    /// `\verb`'s braces and `%` are characters: no group, no comment, and
    /// the paragraph ends where it would without them.
    #[test]
    fn verb() {
        use SyntaxKind::{Brace, Comment, Cs, Newline, Space, Text, Verbatim};
        let src = "a \\verb|{%| b\n\nc \\verb*+}+\n";
        assert_eq!(
            texts(&Tree::parse(src)),
            ["a \\verb|{%| b\n\n", "c \\verb*+}+\n"]
        );
        assert_eq!(
            tokens("a \\verb|{%| b"),
            [
                (Text, "a"),
                (Space, " "),
                (Cs, "\\verb"),
                (Verbatim, "|{%|"),
                (Space, " "),
                (Text, "b")
            ]
        );
        // (no closing delimiter: to the end of the line, as LaTeX reads it
        // before its error)
        assert_eq!(
            tokens("\\verb|{x\n}"),
            [
                (Cs, "\\verb"),
                (Verbatim, "|{x"),
                (Newline, "\n"),
                (Text, "}")
            ]
            .map(|(k, t)| (if t == "}" { SyntaxKind::Other } else { k }, t))
        );
        // (a URL's `%` and `#`; `\href`'s text is read as usual)
        assert_eq!(
            tokens("\\href{a%20#b}{x}%c"),
            [
                (Cs, "\\href"),
                (Verbatim, "{a%20#b}"),
                (Brace, "{"),
                (Text, "x"),
                (Brace, "}"),
                (Comment, "%c")
            ]
        );
        assert_eq!(
            tokens("\\lstinline[language=C]{a{b}%}"),
            [(Cs, "\\lstinline"), (Verbatim, "[language=C]{a{b}%}")]
        );
        // (the lists say what is read)
        assert!(
            VERBATIM_COMMANDS
                .iter()
                .all(|c| verbatim_cmd(c.as_bytes()).is_some())
        );
        assert!(
            VERBATIM_ENVIRONMENTS
                .iter()
                .all(|e| verbatim_env_args(e.as_bytes()).is_some())
        );
        // (a control word that only begins like one is not)
        assert_eq!(tokens("\\verbatim{x}")[0], (Cs, "\\verbatim"));
    }

    /// A verbatim environment's body is one run: its blank lines do not
    /// end the paragraph, its braces open nothing; its arguments are read
    /// as usual.
    #[test]
    fn verbatim_environments() {
        use SyntaxKind::{Brace, Cs, Newline, Text, Verbatim};
        let src = "x\n\\begin{verbatim}\n{\n\n%}\n\\end{verbatim}\ny\n\nz\n";
        assert_eq!(
            texts(&Tree::parse(src)),
            [
                "x\n\\begin{verbatim}\n{\n\n%}\n\\end{verbatim}\ny\n\n",
                "z\n"
            ]
        );
        let toks = tokens("\\begin{lstlisting}[caption={a}]\nint x;{\n\\end{lstlisting}");
        assert_eq!(
            toks,
            [
                (Cs, "\\begin"),
                (Brace, "{"),
                (Text, "lstlisting"),
                (Brace, "}"),
                (Text, "[caption="),
                (Brace, "{"),
                (Text, "a"),
                (Brace, "}"),
                (Text, "]"),
                (Verbatim, "\nint x;{\n"),
                (Cs, "\\end"),
                (Brace, "{"),
                (Text, "lstlisting"),
                (Brace, "}"),
            ]
        );
        assert_eq!(
            tokens("\\begin{minted}{python}\nx\\end{minted}")[6..8],
            [(Brace, "}"), (Verbatim, "\nx")]
        );
        // (an empty body; a body never closed runs to the end)
        assert!(
            tokens("\\begin{comment}\\end{comment}")
                .iter()
                .all(|t| t.0 != Verbatim)
        );
        let src = "a\n\n\\begin{comment}\n\nb\n\nc\n";
        assert_eq!(
            texts(&Tree::parse(src)),
            ["a\n\n", "\\begin{comment}\n\nb\n\nc\n"]
        );
        // (`\begin{verbatim}` in a comment is a comment)
        let src = "% \\begin{verbatim}\n\nb\n";
        assert_eq!(texts(&Tree::parse(src)), ["% \\begin{verbatim}\n\n", "b\n"]);
        assert_eq!(tokens("a\n").last(), Some(&(Newline, "\n")));
    }

    /// Edits that open and close verbatim runs: the tree is the fresh
    /// parse's, and the paragraphs outside the splice are kept.
    #[test]
    fn verbatim_edits() {
        let src = "a\n\nb\n\n\\begin{verbatim}\n{\n\nx\n\\end{verbatim}\n\nc\n\nd\n";
        assert_eq!(Tree::parse(src).paras().len(), 5);
        // the verbatim's end removed: it runs to the end
        let at = src.find("\\end").unwrap();
        check_edit(src, at, 1, "");
        // a `\verb|{|` typed in `b`, then its delimiter removed
        check_edit(src, 3, 0, "\\verb|{|");
        check_edit(src, 3, 0, "\\verb|{");
        // a verbatim opened in `a`, before an existing end
        check_edit(src, 0, 0, "\\begin{verbatim}");
        check_edit(src, 0, 1, "\\begin{comment}");
    }

    /// Random edits of a document made of the pieces that move paragraph
    /// boundaries: the edited tree is always the fresh parse's.
    #[test]
    fn random_edits() {
        const PIECES: &[&str] = &[
            "w",
            " ",
            "\n",
            "\n\n",
            "{",
            "}",
            "%",
            "\\verb|",
            "|",
            "\\begin{verbatim}",
            "\\end{verbatim}",
            "\\url{",
            "\\x",
            "\\begin{lstlisting}[",
            "]",
            "\\end{lstlisting}",
        ];
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rand = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            usize::try_from(seed % (n as u64)).unwrap()
        };
        let mut src = String::new();
        for _ in 0..200 {
            src.push_str(PIECES[rand(PIECES.len())]);
        }
        let mut t = Tree::parse(&src);
        for _ in 0..500 {
            let start = rand(src.len() + 1);
            let old_len = rand((src.len() - start).min(12) + 1);
            let mut text = String::new();
            for _ in 0..rand(3) {
                text.push_str(PIECES[rand(PIECES.len())]);
            }
            let before: Vec<ParaId> = t.paras().iter().map(|p| p.id).collect();
            let s = t.edit(start, old_len, &text);
            src.replace_range(start..start + old_len, &text);
            assert_eq!(t.text(), src);
            let fresh = Tree::parse(&src);
            assert_eq!(t.para_ends(), fresh.para_ends(), "{src:?}");
            for (p, f) in t.paras().iter().zip(fresh.paras()) {
                assert_eq!(p.green, f.green);
            }
            let after: Vec<ParaId> = t.paras().iter().map(|p| p.id).collect();
            assert_eq!(after[..s.at], before[..s.at]);
            assert_eq!(after[s.at + s.inserted..], before[s.at + s.removed..]);
        }
    }
}
