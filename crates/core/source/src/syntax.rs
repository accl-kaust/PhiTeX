//! TeX's shape in raw source, without running TeX: the lexer ([`lex`]:
//! a text as a green tree of tokens and groups, read with plain TeX's
//! catcodes and LaTeX's verbatim constructs) and the paragraph splitter
//! ([`Paragraphs`]: units that end after the blank lines that follow
//! their text at brace depth 0, never inside a verbatim run).
//!
//! The tree is green and red, as in rust-analyzer: a green node knows its
//! kind, its length and its children, never its position; a red node
//! ([`Red`]) is a green node and the offset it is at, computed while
//! walking down. A token is only a kind and a length, inline in its parent:
//! its text is the unit's.
//!
//! LaTeX's verbatim constructs, whose text is characters, not commands,
//! are read as such ([`SyntaxKind::Verbatim`]): `\verb|…|` (and
//! `\lstinline`, `\mintinline`, `\Verb`, `\url`, `\nolinkurl`, the URL of
//! `\href`), and the body of the environments in [`VERBATIM_ENVIRONMENTS`]
//! up to their `\end{name}`. A brace or a `%` in them neither opens a group
//! nor starts a comment, and a blank line in a verbatim body does not end
//! the paragraph.

use crate::{Split, SplitHint};

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

/// Units as paragraphs: runs of lines up to and including the blank lines
/// that end them, at brace depth 0, outside verbatim runs. An edit may
/// move the end of the paragraph before it (a blank line added at its
/// start), so the one before is cut again too.
#[derive(Clone, Copy, Debug, Default)]
pub struct Paragraphs;

impl Split for Paragraphs {
    fn split(&self, text: &str, _: Option<&dyn SplitHint>) -> (Vec<usize>, bool) {
        let b = text.as_bytes();
        let mut ends = Vec::new();
        let (mut pos, mut closed) = (0, true);
        while pos < b.len() {
            (pos, closed) = para_end(b, pos);
            ends.push(pos);
        }
        (ends, closed)
    }

    fn lookbehind(&self) -> usize {
        1
    }
}

fn line_end(s: &[u8], from: usize) -> usize {
    s[from..]
        .iter()
        .position(|&c| c == b'\n')
        .map_or(s.len(), |i| from + i)
}

/// The bytes the paragraph cutter stops at.
static PARA_SPECIAL: [bool; 256] = {
    let mut t = [false; 256];
    t[b'\\' as usize] = true;
    t[b'%' as usize] = true;
    t[b'{' as usize] = true;
    t[b'}' as usize] = true;
    t[b'\n' as usize] = true;
    t
};

/// Where the paragraph that begins at `start` ends: after the blank lines
/// that follow its text at brace depth 0 (the blank lines before any text
/// are its own). A verbatim run is skipped whole: its braces, `%` and blank
/// lines are characters. (One pass over the bytes.)
///
/// And whether the end is sure: the text after it cannot move it. It is
/// when a line with text follows the blank lines, or when the text ends
/// just after a blank line: what follows is a paragraph's first line,
/// which has text (a paragraph never begins with a blank line, but the
/// document's first).
fn para_end(s: &[u8], start: usize) -> (usize, bool) {
    let len = s.len();
    let blank = |c: u8| matches!(c, b' ' | b'\t' | b'\r');
    let mut i = start;
    let mut depth = 0usize;
    let mut text = false;
    loop {
        // (a line from `i`: blank, or with text)
        let mut c = i;
        while c < len && blank(s[c]) {
            c += 1;
        }
        if c >= len {
            return (len, false);
        }
        if s[c] == b'\n' {
            if depth == 0 && text {
                // (the blank lines after it are its own)
                let mut after = c + 1;
                loop {
                    let mut end = after;
                    while end < len && blank(s[end]) {
                        end += 1;
                    }
                    if end >= len {
                        return (len, after == len);
                    }
                    if s[end] != b'\n' {
                        return (after, true);
                    }
                    after = end + 1;
                }
            }
            i = c + 1;
            continue;
        }
        text = true;
        while c < len {
            let ch = s[c];
            if !PARA_SPECIAL[ch as usize] {
                c += 1;
                continue;
            }
            match ch {
                b'\n' => break,
                b'\\' => {
                    if let Some(v) = verbatim_at(s, c) {
                        // (to where the run ends, maybe lines further)
                        c = v.end;
                        continue;
                    }
                    // (the next character is the control symbol's, unless
                    // it ends the line)
                    c += 1;
                    if c < len && s[c] != b'\n' {
                        c += 1;
                    }
                    continue;
                }
                b'%' => {
                    while c < len && s[c] != b'\n' {
                        c += 1;
                    }
                    break;
                }
                b'{' => depth += 1,
                _ => depth = depth.saturating_sub(1),
            }
            c += 1;
        }
        if c >= len {
            return (len, false);
        }
        i = c + 1;
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
    // (the names' first letters: most commands are not one of them)
    if !matches!(
        s.get(i + 1),
        Some(b'b' | b'v' | b'V' | b'l' | b'm' | b'u' | b'n' | b'h')
    ) {
        return None;
    }
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
    // (every child not yet in a node, in order, and where each open
    // group's begin: a group's children move to a vector of their own,
    // of their exact size, when it closes)
    let mut items: Vec<Green> = Vec::with_capacity(b.len() / 4 + 4);
    let mut open: Vec<usize> = Vec::new();
    let mut i = 0;
    // (a verbatim run ahead: the tokens before it stop where it begins)
    let mut verb: Option<Verb> = None;
    let token = |kind, len| Green::Token { kind, len };
    while i < b.len() {
        if let Some(v) = verb.filter(|v| v.start <= i) {
            verb = None;
            if v.end > i {
                items.push(token(SyntaxKind::Verbatim, v.end - i));
                i = v.end;
            }
            continue;
        }
        match b[i] {
            b'{' => {
                open.push(items.len());
                items.push(token(SyntaxKind::Brace, 1));
                i += 1;
            }
            b'}' if !open.is_empty() => {
                items.push(token(SyntaxKind::Brace, 1));
                let from = open.pop().unwrap_or(0);
                let group = node(SyntaxKind::Group, items.drain(from..).collect());
                items.push(group);
                i += 1;
            }
            c => {
                if c == b'\\' && verb.is_none() {
                    verb = verbatim_at(b, i);
                }
                let limit = verb.map_or(b.len(), |v| v.start);
                let (kind, end) = scan(b, i, limit, &Catcodes::PLAIN);
                items.push(token(kind, end - i));
                i = end;
            }
        }
    }
    // (a group still open at the end: closed here, without its brace)
    while let Some(from) = open.pop() {
        let group = node(SyntaxKind::Group, items.drain(from..).collect());
        items.push(group);
    }
    items.shrink_to_fit();
    node(SyntaxKind::Para, items)
}

/// The catcode regime a text is read under, as far as the shape of its
/// tokens goes: which ASCII bytes are letters (catcode 11, and so make up
/// control-sequence names), and whether spaces are ignored (catcode 9, as
/// under expl3's `\ExplSyntaxOn`), which of `\ { } % #` are other
/// characters (or letters), and which other characters are escapes or
/// open or close groups (catcodes 0, 1 and 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Catcodes {
    letters: u128,
    /// Characters that normally shape tokens (`\\`, `{`, `}`, `%`, `#`)
    /// read as other characters (catcode 12).
    other: u128,
    /// Other characters of catcode 1 (begin-group).
    open: u128,
    /// Other characters of catcode 2 (end-group).
    close: u128,
    /// Other characters of catcode 0 (escape).
    escape: u128,
    pub ignore_spaces: bool,
    /// Of `$ & ^ _ ~` and tab: those of catcode 12 (INITEX's, until a
    /// `\catcode` gives them plain TeX's).
    specials12: u128,
    /// Bytes a literal `\catcode` set to a value this regime does not
    /// model: their catcode is not known here.
    unknown: u128,
}

/// `$ & ^ _ ~` and tab: catcode 12 under INITEX, their own under plain.
const SPECIALS: u128 = 1 << b'$' | 1 << b'&' | 1 << b'^' | 1 << b'_' | 1 << b'~' | 1 << b'\t';

impl Catcodes {
    /// Plain TeX's and a LaTeX document's: the letters are `a`–`z`, `A`–`Z`.
    pub const PLAIN: Catcodes = Catcodes {
        letters: ((1u128 << 26) - 1) << b'A' | ((1u128 << 26) - 1) << b'a',
        other: 0,
        open: 0,
        close: 0,
        escape: 0,
        ignore_spaces: false,
        specials12: 0,
        unknown: 0,
    };

    /// INITEX's (tex.web §232): letters, `\\`, `%`, space; everything
    /// else other, until the text's own `\catcode` lines.
    #[must_use]
    pub const fn initex() -> Catcodes {
        let mut r = Catcodes::PLAIN
            .with_other(b'{', true)
            .with_other(b'}', true)
            .with_other(b'#', true);
        r.specials12 = SPECIALS;
        r
    }

    /// This regime with `c` (one of `$ & ^ _ ~` or tab) of catcode `v`:
    /// plain TeX's for it, 12, or one not modelled here.
    #[must_use]
    pub const fn with_special(self, c: u8, v: u32) -> Catcodes {
        let mut r = self;
        if c < 128 && SPECIALS >> c & 1 == 1 {
            let plain = match c {
                b'$' => 3,
                b'&' => 4,
                b'^' => 7,
                b'_' => 8,
                b'~' => 13,
                _ => 10,
            };
            r.specials12 &= !(1 << c);
            r.unknown &= !(1 << c);
            if v == 12 {
                r.specials12 |= 1 << c;
            } else if v != plain {
                r.unknown |= 1 << c;
            }
        }
        r
    }

    /// This regime with `c`'s catcode not known (`on`), or known again.
    #[must_use]
    pub const fn with_unknown(self, c: u8, on: bool) -> Catcodes {
        let mut r = self;
        if c < 128 {
            if on {
                r.unknown |= 1 << c;
            } else {
                r.unknown &= !(1 << c);
            }
        }
        r
    }

    /// Whether `c` (one of `$ & ^ _ ~` or tab) is an other character.
    #[must_use]
    pub const fn is_special12(&self, c: u8) -> bool {
        c < 128 && self.specials12 >> c & 1 == 1
    }

    /// Whether `c`'s catcode is not known to this regime.
    #[must_use]
    pub const fn is_unknown(&self, c: u8) -> bool {
        c < 128 && self.unknown >> c & 1 == 1
    }

    /// LaTeX's internal regime (`\makeatletter`, `.sty` and `.cls` files):
    /// `@` is a letter.
    #[must_use]
    pub const fn at_letter() -> Catcodes {
        Catcodes::PLAIN.with_letter(b'@')
    }

    /// expl3's (`\ExplSyntaxOn`): `_` and `:` are letters and spaces are
    /// ignored (`@` keeps what it was: a letter in a package).
    #[must_use]
    pub const fn expl(self) -> Catcodes {
        Catcodes {
            letters: self.with_letter(b'_').with_letter(b':').letters,
            other: self.other,
            open: self.open,
            close: self.close,
            escape: self.escape,
            ignore_spaces: true,
            specials12: self.specials12 & !(1 << b'_'),
            unknown: self.unknown & !(1 << b'_'),
        }
    }

    /// This regime with `c` a letter.
    #[must_use]
    pub const fn with_letter(self, c: u8) -> Catcodes {
        let mut r = self;
        if c < 128 {
            r.letters |= 1 << c;
        }
        r
    }

    /// This regime with `c` not a letter.
    #[must_use]
    pub const fn without_letter(self, c: u8) -> Catcodes {
        let mut r = self;
        if c < 128 {
            r.letters &= !(1 << c);
        }
        r
    }

    /// This regime with `c` (one of `\\ { } % #`) an other character, or
    /// not.
    #[must_use]
    pub const fn with_other(self, c: u8, on: bool) -> Catcodes {
        let mut r = self;
        if c < 128 {
            if on {
                r.other |= 1 << c;
            } else {
                r.other &= !(1 << c);
            }
        }
        r
    }

    /// This regime with `c` (not one of `\\ { } % #`) of catcode `v`
    /// when it is 0, 1 or 2, or of none of them.
    #[must_use]
    pub const fn with_shaping(self, c: u8, v: u32) -> Catcodes {
        let mut r = self;
        if c < 128 {
            r.open &= !(1 << c);
            r.close &= !(1 << c);
            r.escape &= !(1 << c);
            match v {
                0 => r.escape |= 1 << c,
                1 => r.open |= 1 << c,
                2 => r.close |= 1 << c,
                _ => {}
            }
        }
        r
    }

    /// Whether `c` is an escape character in place of `\\`.
    #[must_use]
    pub const fn is_escape(&self, c: u8) -> bool {
        c < 128 && self.escape >> c & 1 == 1
    }

    /// Whether `c` (not one of `\\ { } % #`) is an escape, or opens or
    /// closes a group.
    #[must_use]
    pub const fn is_shaping(&self, c: u8) -> bool {
        c < 128 && (self.open | self.close | self.escape) >> c & 1 == 1
    }

    /// Whether `c` opens a group in place of `{`.
    #[must_use]
    pub const fn is_open(&self, c: u8) -> bool {
        c < 128 && self.open >> c & 1 == 1
    }

    /// Whether `c` (not `{` or `}`) closes a group.
    #[must_use]
    pub const fn is_close(&self, c: u8) -> bool {
        c < 128 && self.close >> c & 1 == 1
    }

    #[must_use]
    pub const fn is_other(&self, c: u8) -> bool {
        c < 128 && self.other >> c & 1 == 1
    }

    #[must_use]
    pub const fn is_letter(&self, c: u8) -> bool {
        c < 128 && self.letters >> c & 1 == 1
    }
}

/// The token at `i` under `cat`, read no further than `limit`: its kind
/// and where it ends. A `{` or a `}` is a [`SyntaxKind::Brace`] of one
/// byte; verbatim runs are not looked for ([`verbatim_run`] finds them).
#[must_use]
pub(crate) fn token_at(text: &str, i: usize, limit: usize, cat: &Catcodes) -> (SyntaxKind, usize) {
    let b = text.as_bytes();
    match b[i] {
        b'{' | b'}' if !cat.is_other(b[i]) => (SyntaxKind::Brace, i + 1),
        c if cat.is_open(c) || cat.is_close(c) => (SyntaxKind::Brace, i + 1),
        _ => scan(b, i, limit.min(b.len()), cat),
    }
}

/// The verbatim run that the command at `i` (a backslash) begins, if it is
/// one of LaTeX's verbatim constructs: the bytes from after its name to the
/// end of its verbatim text (a verbatim environment's: its body).
#[must_use]
pub(crate) fn verbatim_run(text: &str, i: usize) -> Option<std::ops::Range<usize>> {
    verbatim_at(text.as_bytes(), i).map(|v| v.start..v.end)
}

/// The bytes that end a text token.
static TEXT_END: [bool; 256] = {
    let mut t = [false; 256];
    let mut k = 0;
    let ends = b"\\{}%\n \t\r#";
    while k < ends.len() {
        t[ends[k] as usize] = true;
        k += 1;
    }
    t
};

/// The token at `i` (not a group's brace), read no further than `limit`:
/// its kind and where it ends.
fn scan(b: &[u8], mut i: usize, limit: usize, cat: &Catcodes) -> (SyntaxKind, usize) {
    let kind = match b[i] {
        c if cat.other != 0 && cat.is_other(c) => {
            i += 1;
            while i < limit
                && (!TEXT_END[b[i] as usize] || cat.is_other(b[i]))
                && !cat.is_shaping(b[i])
            {
                i += 1;
            }
            SyntaxKind::Text
        }
        c if c == b'\\' || cat.is_escape(c) => {
            i += 1;
            if i < limit {
                if cat.is_letter(b[i]) {
                    while i < limit && cat.is_letter(b[i]) {
                        i += 1;
                    }
                } else if b[i] == b'^'
                    && !cat.is_other(b'^')
                    && i + 2 < limit
                    && b[i + 1] == b'^'
                    && b[i + 2] < 128
                {
                    // (`\^^M`: the character `^^` stands for is the
                    // symbol's name, §355)
                    let hex = |c: u8| c.is_ascii_digit() || (b'a'..=b'f').contains(&c);
                    i += if i + 3 < limit && hex(b[i + 2]) && hex(b[i + 3]) { 4 } else { 3 };
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
                && (!TEXT_END[b[i] as usize] || cat.other != 0 && cat.is_other(b[i]))
                && !cat.is_shaping(b[i])
            {
                i += 1;
            }
            SyntaxKind::Text
        }
    };
    (kind, i)
}

#[cfg(test)]
mod tests;
