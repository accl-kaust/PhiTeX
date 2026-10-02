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
/// are its own).
fn para_end(s: &[u8], start: usize) -> usize {
    let mut i = start;
    let mut depth = 0usize;
    let mut text = false;
    loop {
        if i >= s.len() {
            return s.len();
        }
        let e = line_end(s, i);
        let next = (e + 1).min(s.len());
        if blank(&s[i..e]) {
            if depth == 0 && text {
                let mut j = next;
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
                    b'\\' => c += 1,
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
        i = next;
    }
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

/// A paragraph's text as a green node, read with plain TeX's catcodes.
fn lex(text: &str) -> Green {
    let token = |kind, len| Green::Token { kind, len };
    let b = text.as_bytes();
    let mut stack: Vec<(SyntaxKind, Vec<Green>)> = vec![(SyntaxKind::Para, Vec::new())];
    let mut i = 0;
    while i < b.len() {
        let start = i;
        let kind = match b[i] {
            b'\\' => {
                i += 1;
                if i < b.len() {
                    if b[i].is_ascii_alphabetic() {
                        while i < b.len() && b[i].is_ascii_alphabetic() {
                            i += 1;
                        }
                    } else {
                        i = (i + utf8_len(b[i])).min(b.len());
                    }
                }
                SyntaxKind::Cs
            }
            b'{' => {
                stack.push((SyntaxKind::Group, vec![token(SyntaxKind::Brace, 1)]));
                i += 1;
                continue;
            }
            b'}' => {
                i += 1;
                if stack.len() > 1 {
                    let (kind, mut children) = stack.pop().expect("a group");
                    children.push(token(SyntaxKind::Brace, 1));
                    stack
                        .last_mut()
                        .expect("the paragraph")
                        .1
                        .push(node(kind, children));
                    continue;
                }
                SyntaxKind::Other
            }
            b'%' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                SyntaxKind::Comment
            }
            b'\n' => {
                i += 1;
                SyntaxKind::Newline
            }
            b' ' | b'\t' | b'\r' => {
                while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\r') {
                    i += 1;
                }
                SyntaxKind::Space
            }
            b'#' => {
                i += 1;
                if i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
                SyntaxKind::Param
            }
            _ => {
                while i < b.len()
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
        stack
            .last_mut()
            .expect("the paragraph")
            .1
            .push(token(kind, i - start));
    }
    // (a group still open at the end: closed here, without its brace)
    while stack.len() > 1 {
        let (kind, children) = stack.pop().expect("a group");
        stack
            .last_mut()
            .expect("the paragraph")
            .1
            .push(node(kind, children));
    }
    let (kind, children) = stack.pop().expect("the paragraph");
    node(kind, children)
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
}
