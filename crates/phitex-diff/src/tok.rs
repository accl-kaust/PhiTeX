//! The diff's tokens: a document's body, read from its CST, as a tree of
//! words, commands with their arguments, math units and environments.
//!
//! The CST gives the characters' kinds and the brace groups; the signature
//! table says which arguments follow a command and what it is; this module
//! matches `\begin` with `\end`, finds math, and attaches each token's
//! trailing white space to it (a blank line is a token of its own).

use crate::sig::{Class, EnvKind, Signatures};
use phitex_syntax::{Green, SyntaxKind as K, Tree, VERBATIM_COMMANDS, VERBATIM_ENVIRONMENTS};
use std::hash::{DefaultHasher, Hash, Hasher};

/// A lexeme: a CST token, with text split into words and single
/// characters, and groups nested. Ranges are absolute byte offsets.
#[derive(Clone, Debug)]
pub enum Lx {
    Cs(usize, usize),
    Word(usize, usize),
    Char(usize, usize),
    Space(usize, usize),
    Nl(usize, usize),
    Comment(usize, usize),
    /// A brace group: its whole range, where its content is, and the
    /// content.
    Group {
        start: usize,
        end: usize,
        inner: (usize, usize),
        kids: Vec<Lx>,
    },
    /// A parameter, a verbatim run, a stray `}`.
    Other(usize, usize),
    Verb(usize, usize),
}

impl Lx {
    #[must_use]
    pub fn range(&self) -> (usize, usize) {
        match self {
            Lx::Cs(a, b)
            | Lx::Word(a, b)
            | Lx::Char(a, b)
            | Lx::Space(a, b)
            | Lx::Nl(a, b)
            | Lx::Comment(a, b)
            | Lx::Verb(a, b)
            | Lx::Other(a, b) => (*a, *b),
            Lx::Group { start, end, .. } => (*start, *end),
        }
    }
}

/// The lexemes of `text[a..b]`, read as the CST reads them, paragraph by
/// paragraph.
#[must_use]
pub fn lexemes(text: &str, a: usize, b: usize) -> Vec<Lx> {
    let tree = Tree::parse(&text[a..b]);
    let mut out = Vec::new();
    for r in tree.reds() {
        put(r.green.children(), a + r.offset, text, &mut out);
    }
    out
}

/// The lexemes of paragraphs `paras` (CST paragraph nodes, whose text is
/// `text` from its start), as [`lexemes`] reads them in a whole text.
pub fn lexemes_of<'g>(paras: impl IntoIterator<Item = &'g Green>, text: &str) -> Vec<Lx> {
    let mut out = Vec::new();
    let mut at = 0;
    for g in paras {
        put(g.children(), at, text, &mut out);
        at += g.len();
    }
    out
}

/// The lexemes of green nodes `kids` at `at`.
fn put(kids: &[Green], at: usize, text: &str, out: &mut Vec<Lx>) {
    let mut pos = at;
    for c in kids {
        let (a, b) = (pos, pos + c.len());
        pos = b;
        match c.kind() {
            K::Cs => out.push(Lx::Cs(a, b)),
            K::Space => out.push(Lx::Space(a, b)),
            K::Newline => out.push(Lx::Nl(a, b)),
            K::Comment => out.push(Lx::Comment(a, b)),
            K::Verbatim => out.push(Lx::Verb(a, b)),
            K::Param | K::Other | K::Brace | K::Para => out.push(Lx::Other(a, b)),
            K::Text => {
                let s = &text[a..b];
                let mut word: Option<usize> = None;
                let bytes = s.as_bytes();
                for (k, ch) in s.char_indices() {
                    // (a number's point or comma is in it: 1.5, 1,000)
                    let inner = matches!(ch, '.' | ',')
                        && k > 0
                        && bytes[k - 1].is_ascii_digit()
                        && bytes.get(k + 1).is_some_and(u8::is_ascii_digit);
                    if ch.is_alphanumeric() || (inner && word.is_some()) {
                        word.get_or_insert(a + k);
                    } else {
                        if let Some(w) = word.take() {
                            out.push(Lx::Word(w, a + k));
                        }
                        out.push(Lx::Char(a + k, a + k + ch.len_utf8()));
                    }
                }
                if let Some(w) = word {
                    out.push(Lx::Word(w, b));
                }
            }
            K::Group => {
                let g = c.children();
                // (the braces are the group's; one still open at the end
                // has no closing brace)
                let closed = g.len() >= 2 && g.last().is_some_and(|k| k.kind() == K::Brace);
                let inner_kids = &g[1..g.len() - usize::from(closed)];
                let inner = (a + 1, if closed { b - 1 } else { b });
                let mut sub = Vec::new();
                put(inner_kids, a + 1, text, &mut sub);
                out.push(Lx::Group {
                    start: a,
                    end: b,
                    inner,
                    kids: sub,
                });
            }
        }
    }
}

/// What a token is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A word or a character: can sit inside the markup.
    Word,
    /// White space no other token took (at the start of a sequence).
    Ws,
    /// A safe command with its arguments.
    Safe,
    /// A command safe in an `\mbox`.
    Mbox,
    /// Anything that cannot sit inside the markup.
    Unsafe,
    /// An alignment tab.
    Amp,
    /// A row's end (`\\`) in an alignment.
    RowEnd,
    /// A rule in an alignment (`\hline`).
    Rule,
    Comment,
    /// A blank line (or more).
    Par,
    /// Inline math, a unit.
    Math,
    /// Display math, a unit: its environment (empty for `\[`, `$$`), whether
    /// it aligns, and its body's range.
    Display {
        env: String,
        arr: bool,
        body: (usize, usize),
    },
    /// A picture or a verbatim environment: a unit, no markup inside.
    Picture,
    /// A command whose last argument is text, diffed inside.
    Text {
        name: String,
        class: Class,
        star: bool,
    },
    /// A brace group, diffed inside.
    Group,
    Env {
        name: String,
        kind: EnvKind,
    },
    /// A row of an alignment.
    Row,
}

/// A token: its kind, where it is (`start..end` with its trailing white
/// space; `cend` where its content ends), its key (its text, white space
/// normalized) and its prefix key (what must be the same for its insides
/// to be diffed: for a command its name and other arguments), and for a
/// node its children, between its open part (`start..open_end`) and its
/// close part (`close_start..end`).
#[derive(Clone, Debug)]
pub struct Tok {
    pub kind: Kind,
    pub start: usize,
    pub cend: usize,
    pub end: usize,
    pub key: u64,
    pub prefix: u64,
    pub open_end: usize,
    pub close_start: usize,
    pub children: Vec<Tok>,
}

impl Tok {
    #[must_use]
    pub fn is_node(&self) -> bool {
        matches!(
            self.kind,
            Kind::Text { .. } | Kind::Group | Kind::Env { .. } | Kind::Row
        )
    }
}

/// Hash `s` with its white space runs as one space, none at the ends.
pub fn norm_hash(s: &str, h: &mut DefaultHasher) {
    let mut space = false;
    let mut any = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            space = any;
        } else {
            if space {
                ' '.hash(h);
                space = false;
            }
            any = true;
            ch.hash(h);
        }
    }
}

fn key_of(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    norm_hash(s, &mut h);
    h.finish()
}

/// The sectioning commands (for the change list's sections).
pub const SECTIONS: &[&str] = &["part", "chapter", "section", "subsection", "subsubsection"];

/// The rules of an alignment.
const RULES: &[&str] = &[
    "\\hline",
    "\\cline",
    "\\toprule",
    "\\midrule",
    "\\bottomrule",
    "\\cmidrule",
    "\\addlinespace",
    "\\specialrule",
    "\\hhline",
];

/// The text commands that may sit in the markup whole when their text is
/// not a group (`\emph x`).
fn safe_text(name: &str) -> bool {
    matches!(name, "emph" | "mbox" | "fbox") || name.starts_with("text")
}

/// The tokenizer: over a text, with a signature table.
pub struct Reader<'a> {
    pub text: &'a str,
    pub sigs: &'a Signatures,
}

/// A position in a lexeme slice: the index, and the bytes of a word there
/// already read (a one-character argument taken from it).
#[derive(Clone, Copy, Debug)]
struct Pos {
    i: usize,
    sub: usize,
}

impl Pos {
    fn at(i: usize) -> Pos {
        Pos { i, sub: 0 }
    }
}

/// A node's parts.
struct Node {
    open_end: usize,
    close_start: usize,
    children: Vec<Tok>,
    prefix: u64,
}

impl<'a> Reader<'a> {
    fn s(&self, a: usize, b: usize) -> &'a str {
        &self.text[a..b]
    }

    /// The tokens of `lx`.
    #[must_use]
    pub fn seq(&self, lx: &[Lx]) -> Vec<Tok> {
        let mut out = Vec::new();
        let mut p = Pos::at(0);
        while p.i < lx.len() {
            let start = lx[p.i].range().0 + p.sub;
            let (kind, q, node) = self.one(lx, p);
            let cend = if q.sub > 0 {
                lx[q.i].range().0 + q.sub
            } else {
                lx[q.i - 1].range().1
            };
            let (q, end) = match kind {
                Kind::Par | Kind::Ws => (q, cend),
                Kind::Comment => ws_after(lx, q, cend, true),
                _ => ws_after(lx, q, cend, false),
            };
            let key = match kind {
                // (verbatim: its white space is its text)
                Kind::Picture | Kind::Unsafe if self.s(start, cend).contains("\\begin") => {
                    let mut h = DefaultHasher::new();
                    self.s(start, cend).hash(&mut h);
                    h.finish()
                }
                // (a blank line is not white space: it ends a paragraph)
                Kind::Par => key_of("\n\u{2029}"),
                _ => key_of(self.s(start, cend)),
            };
            let mut t = Tok {
                kind,
                start,
                cend,
                end,
                key,
                prefix: key,
                open_end: start,
                close_start: end,
                children: Vec::new(),
            };
            if let Some(n) = node {
                t.open_end = n.open_end;
                t.close_start = n.close_start;
                t.children = n.children;
                t.prefix = n.prefix;
            }
            out.push(t);
            p = q;
        }
        out
    }

    /// The token at `p`: its kind, where it ends, and its node parts.
    fn one(&self, lx: &[Lx], p: Pos) -> (Kind, Pos, Option<Node>) {
        let next = Pos::at(p.i + 1);
        if p.sub > 0 {
            return (Kind::Word, next, None);
        }
        match &lx[p.i] {
            Lx::Space(..) | Lx::Nl(..) => {
                let mut q = p.i;
                while q < lx.len() && matches!(lx[q], Lx::Space(..) | Lx::Nl(..)) {
                    q += 1;
                }
                let nls = lx[p.i..q]
                    .iter()
                    .filter(|l| matches!(l, Lx::Nl(..)))
                    .count();
                // (at a line's start, after a comment, one line end is a
                // blank line)
                let mut k = p.i;
                while k > 0 && matches!(lx[k - 1], Lx::Space(..)) {
                    k -= 1;
                }
                let line_start = k > 0 && matches!(lx[k - 1], Lx::Nl(..) | Lx::Comment(..));
                if nls >= 2 || (nls == 1 && line_start) {
                    // (a blank line ends at its last line end, where the CST
                    // cuts a paragraph: the white space after it begins the
                    // next one, so that a paragraph reads alike alone)
                    let last = (p.i..q)
                        .rev()
                        .find(|&k| matches!(lx[k], Lx::Nl(..)))
                        .unwrap_or(q - 1);
                    (Kind::Par, Pos::at(last + 1), None)
                } else {
                    (Kind::Ws, Pos::at(q), None)
                }
            }
            Lx::Word(..) => (Kind::Word, next, None),
            Lx::Char(a, b) => match self.s(*a, *b) {
                "$" => self.dollar(lx, p),
                "&" => (Kind::Amp, next, None),
                // (math characters out of math: not in the markup)
                "^" | "_" => (Kind::Unsafe, next, None),
                _ => (Kind::Word, next, None),
            },
            Lx::Comment(..) => {
                // (with its line end: the comment eats it)
                let mut q = p.i + 1;
                if q < lx.len() && matches!(lx[q], Lx::Nl(..)) {
                    q += 1;
                }
                (Kind::Comment, Pos::at(q), None)
            }
            Lx::Group {
                start, inner, kids, ..
            } => {
                let node = self.node(*start, *inner, kids, "group");
                (Kind::Group, next, Some(node))
            }
            Lx::Cs(a, b) => self.command(lx, p, *a, *b),
            Lx::Verb(..) | Lx::Other(..) => (Kind::Unsafe, next, None),
        }
    }

    /// A node whose content is group `inner` (lexemes `kids`), opened at
    /// `start`.
    fn node(&self, start: usize, inner: (usize, usize), kids: &[Lx], tag: &str) -> Node {
        let (open_end, k0) = lead_ws(kids, 0, kids.len(), inner.0);
        let mut h = DefaultHasher::new();
        tag.hash(&mut h);
        norm_hash(self.s(start, inner.0), &mut h);
        Node {
            open_end,
            close_start: inner.1,
            children: self.seq(&kids[k0..]),
            prefix: h.finish(),
        }
    }

    /// `$…$` or `$$…$$` at `p`.
    fn dollar(&self, lx: &[Lx], p: Pos) -> (Kind, Pos, Option<Node>) {
        let is_dollar = |l: &Lx| matches!(l, Lx::Char(a, b) if self.s(*a, *b) == "$");
        let double = lx.get(p.i + 1).is_some_and(is_dollar);
        let open = if double { 2 } else { 1 };
        let mut q = p.i + open;
        while q < lx.len() {
            if is_dollar(&lx[q]) && (!double || lx.get(q + 1).is_some_and(is_dollar)) {
                let body = (lx[p.i + open - 1].range().1, lx[q].range().0);
                let end = Pos::at(q + open);
                let kind = if double {
                    Kind::Display {
                        env: String::new(),
                        arr: false,
                        body,
                    }
                } else {
                    Kind::Math
                };
                return (kind, end, None);
            }
            // (a blank line ends a paragraph: TeX would stop there)
            if matches!(lx[q], Lx::Nl(..))
                && lx[q + 1..]
                    .iter()
                    .find(|l| !matches!(l, Lx::Space(..)))
                    .is_some_and(|l| matches!(l, Lx::Nl(..)))
            {
                break;
            }
            q += 1;
        }
        (Kind::Unsafe, Pos::at(p.i + 1), None)
    }

    /// The command at `p` (`text[a..b]`), with its arguments.
    #[allow(clippy::many_single_char_names)]
    fn command(&self, lx: &[Lx], p: Pos, a: usize, b: usize) -> (Kind, Pos, Option<Node>) {
        let next = Pos::at(p.i + 1);
        let cs = self.s(a, b);
        let name = &cs[1..];
        match name {
            "begin" => return self.environment(lx, p),
            "[" | "(" => {
                let close = if name == "[" { "\\]" } else { "\\)" };
                return match find_cs(lx, p.i + 1, self.text, close) {
                    Some(q) if name == "[" => (
                        Kind::Display {
                            env: String::new(),
                            arr: false,
                            body: (b, lx[q].range().0),
                        },
                        Pos::at(q + 1),
                        None,
                    ),
                    Some(q) => (Kind::Math, Pos::at(q + 1), None),
                    None => (Kind::Unsafe, next, None),
                };
            }
            _ => {}
        }
        // (a verbatim command: its run follows)
        if VERBATIM_COMMANDS.contains(&name) && name != "href" {
            let mut q = p.i + 1;
            while q < lx.len() && !matches!(lx[q], Lx::Verb(..)) && q < p.i + 4 {
                q += 1;
            }
            let q = if q < lx.len() && matches!(lx[q], Lx::Verb(..)) {
                q + 1
            } else {
                p.i + 1
            };
            return (Kind::Unsafe, Pos::at(q), None);
        }
        let Some(sig) = self.sigs.cmd(name) else {
            // (unknown: the groups and brackets right after it)
            let q = greedy_args(lx, p.i + 1, self.text);
            let kind = if RULES.contains(&cs) {
                Kind::Rule
            } else {
                Kind::Unsafe
            };
            return (kind, Pos::at(q), None);
        };
        let (args, q) = read_args(lx, p.i + 1, &sig.spec, self.text);
        let kind = match sig.class {
            Class::Safe => Kind::Safe,
            Class::Mbox => Kind::Mbox,
            Class::Unsafe if cs == "\\\\" || cs == "\\tabularnewline" => Kind::RowEnd,
            Class::Unsafe if RULES.contains(&cs) => Kind::Rule,
            Class::Unsafe => Kind::Unsafe,
            Class::Text { .. } | Class::Context1 | Class::Context2 => {
                // (the text: the last mandatory argument, if a group)
                let last = args.iter().rev().find_map(|a| match a {
                    Arg::Mandatory(i, 0) => Some(*i),
                    _ => None,
                });
                if let Some(gi) = last
                    && q.sub == 0
                    && gi + 1 == q.i
                    && let Lx::Group {
                        start, inner, kids, ..
                    } = &lx[gi]
                {
                    let mut node = self.node(*start, *inner, kids, "text");
                    // (the prefix: the command and its other arguments)
                    let mut h = DefaultHasher::new();
                    "text".hash(&mut h);
                    norm_hash(self.s(a, *start), &mut h);
                    node.prefix = h.finish();
                    let star = args.iter().any(|a| matches!(a, Arg::Star));
                    return (
                        Kind::Text {
                            name: name.to_owned(),
                            class: sig.class,
                            star,
                        },
                        q,
                        Some(node),
                    );
                }
                if safe_text(name) {
                    Kind::Safe
                } else {
                    Kind::Unsafe
                }
            }
        };
        (kind, q, None)
    }

    /// `\begin{name}` at `p`, up to its `\end{name}`.
    fn environment(&self, lx: &[Lx], p: Pos) -> (Kind, Pos, Option<Node>) {
        let next = Pos::at(p.i + 1);
        let mut q = p.i + 1;
        while q < lx.len() && matches!(lx[q], Lx::Space(..)) {
            q += 1;
        }
        let Some(Lx::Group { inner, .. }) = lx.get(q) else {
            return (Kind::Unsafe, next, None);
        };
        let name = self.s(inner.0, inner.1).trim().to_owned();
        let Some(close) = find_end(lx, q + 1, self.text, &name) else {
            // (no `\end`: the `\begin` alone)
            return (Kind::Unsafe, Pos::at(q + 1), None);
        };
        // (the `\end`'s group)
        let mut after = close + 1;
        while after < lx.len() && matches!(lx[after], Lx::Space(..)) {
            after += 1;
        }
        let after = Pos::at(after + 1);
        if VERBATIM_ENVIRONMENTS.contains(&name.as_str()) {
            return (Kind::Picture, after, None);
        }
        let sig = self.sigs.env(&name).cloned();
        let args_end = match &sig {
            Some(s) => read_args(lx, q + 1, &s.spec, self.text).1.i,
            None => greedy_args(lx, q + 1, self.text),
        }
        .min(close);
        let kind = sig.as_ref().map_or(EnvKind::Plain, |s| s.kind);
        let body = (lx[args_end].range().0, lx[close].range().0);
        let body = if args_end == close {
            (body.1, body.1)
        } else {
            body
        };
        match kind {
            EnvKind::Math | EnvKind::MathArr => (
                Kind::Display {
                    env: name,
                    arr: kind == EnvKind::MathArr,
                    body,
                },
                after,
                None,
            ),
            EnvKind::InlineMath => (Kind::Math, after, None),
            EnvKind::Picture => (Kind::Picture, after, None),
            _ => {
                let start = lx[p.i].range().0;
                let at = lx[args_end].range().0;
                let (open_end, k0) = lead_ws(lx, args_end, close, at);
                let kids = self.seq(&lx[k0..close]);
                let children = if kind == EnvKind::Tabular {
                    self.rows(kids)
                } else {
                    kids
                };
                let mut h = DefaultHasher::new();
                "env".hash(&mut h);
                norm_hash(self.s(start, at), &mut h);
                let node = Node {
                    open_end,
                    close_start: lx[close].range().0,
                    children,
                    prefix: h.finish(),
                };
                (Kind::Env { name, kind }, after, Some(node))
            }
        }
    }

    /// An alignment's tokens grouped into rows: each up to and including
    /// its `\\`; rules before a row's cells are its own.
    fn rows(&self, kids: Vec<Tok>) -> Vec<Tok> {
        let mut rows = Vec::new();
        let mut cur: Vec<Tok> = Vec::new();
        for k in kids {
            let row_end = k.kind == Kind::RowEnd;
            // (a rule after a row's cells begins the next row)
            if k.kind == Kind::Rule
                && cur
                    .iter()
                    .any(|c| !matches!(c.kind, Kind::Rule | Kind::Comment | Kind::Ws))
            {
                rows.push(self.row(std::mem::take(&mut cur)));
            }
            cur.push(k);
            if row_end {
                rows.push(self.row(std::mem::take(&mut cur)));
            }
        }
        if !cur.is_empty() {
            rows.push(self.row(cur));
        }
        rows
    }

    fn row(&self, kids: Vec<Tok>) -> Tok {
        let start = kids[0].start;
        let cend = kids.last().map_or(start, |k| k.cend);
        let end = kids.last().map_or(start, |k| k.end);
        let mut h = DefaultHasher::new();
        "row".hash(&mut h);
        kids.iter()
            .filter(|k| k.kind == Kind::Amp)
            .count()
            .hash(&mut h);
        for k in kids.iter().filter(|k| k.kind == Kind::Rule) {
            k.key.hash(&mut h);
        }
        Tok {
            kind: Kind::Row,
            start,
            cend,
            end,
            key: key_of(self.s(start, cend)),
            prefix: h.finish(),
            open_end: start,
            close_start: end,
            children: kids,
        }
    }
}

/// An argument read: a star, a bracket run, or a mandatory one (the lexeme
/// index, and for a word the bytes of it taken).
#[derive(Debug)]
enum Arg {
    Star,
    Opt,
    Mandatory(usize, usize),
}

/// Read arguments by `spec` from `i`: what was read, and where it ends.
#[allow(clippy::many_single_char_names)]
fn read_args(lx: &[Lx], i: usize, spec: &str, text: &str) -> (Vec<Arg>, Pos) {
    let mut args = Vec::new();
    let mut p = Pos::at(i);
    for c in spec.chars() {
        match c {
            's' => {
                if let Some(Lx::Char(a, b)) = lx.get(p.i)
                    && &text[*a..*b] == "*"
                {
                    args.push(Arg::Star);
                    p.i += 1;
                }
            }
            'o' => {
                let q = skip_ws(lx, p.i);
                if let Some(Lx::Char(a, b)) = lx.get(q)
                    && &text[*a..*b] == "["
                    && let Some(e) = find_char(lx, q + 1, text, "]")
                {
                    args.push(Arg::Opt);
                    p.i = e + 1;
                }
            }
            _ => {
                let q = skip_ws(lx, p.i);
                match lx.get(q) {
                    Some(Lx::Group { .. } | Lx::Cs(..) | Lx::Verb(..)) => {
                        args.push(Arg::Mandatory(q, 0));
                        p.i = q + 1;
                    }
                    Some(Lx::Char(a, b)) if !matches!(&text[*a..*b], "}" | "&" | "$" | "[") => {
                        args.push(Arg::Mandatory(q, 0));
                        p.i = q + 1;
                    }
                    Some(Lx::Word(a, b)) => {
                        let first = text[*a..*b].chars().next().map_or(1, char::len_utf8);
                        args.push(Arg::Mandatory(q, first));
                        // (a word cut: the rest is the next token)
                        if *a + first >= *b {
                            p.i = q + 1;
                        } else {
                            return (args, Pos { i: q, sub: first });
                        }
                    }
                    _ => return (args, p),
                }
            }
        }
    }
    (args, p)
}

/// Past white space (spaces and one line end, not a blank line).
fn skip_ws(lx: &[Lx], mut i: usize) -> usize {
    let mut nl = false;
    while i < lx.len() {
        match lx[i] {
            Lx::Space(..) => i += 1,
            Lx::Nl(..) if !nl => {
                nl = true;
                i += 1;
            }
            _ => break,
        }
    }
    i
}

/// The groups and `[…]` right after `i` (no space between): an unknown
/// command's arguments. Where they end.
fn greedy_args(lx: &[Lx], mut i: usize, text: &str) -> usize {
    loop {
        match lx.get(i) {
            Some(Lx::Group { .. }) => i += 1,
            Some(Lx::Char(a, b)) if &text[*a..*b] == "[" => match find_char(lx, i + 1, text, "]") {
                Some(e) => i = e + 1,
                None => return i,
            },
            Some(Lx::Char(a, b)) if &text[*a..*b] == "*" => i += 1,
            _ => return i,
        }
    }
}

/// The next `c` from `i` on this level, before a blank line.
fn find_char(lx: &[Lx], i: usize, text: &str, c: &str) -> Option<usize> {
    for k in i..lx.len() {
        match lx[k] {
            Lx::Char(a, b) if &text[a..b] == c => return Some(k),
            Lx::Nl(..) if lx.get(k + 1).is_some_and(|l| matches!(l, Lx::Nl(..))) => return None,
            _ => {}
        }
    }
    None
}

/// The next control sequence `cs` from `i` on this level.
fn find_cs(lx: &[Lx], i: usize, text: &str, cs: &str) -> Option<usize> {
    (i..lx.len()).find(|&k| matches!(lx[k], Lx::Cs(a, b) if &text[a..b] == cs))
}

/// The `\end{name}` that closes an environment whose content begins at
/// `i` (nested ones of the same name skipped).
fn find_end(lx: &[Lx], i: usize, text: &str, name: &str) -> Option<usize> {
    let group_name = |k: usize| -> Option<&str> {
        let mut q = k + 1;
        while q < lx.len() && matches!(lx[q], Lx::Space(..)) {
            q += 1;
        }
        match lx.get(q) {
            Some(Lx::Group { inner, .. }) => Some(text[inner.0..inner.1].trim()),
            _ => None,
        }
    };
    let mut depth = 0usize;
    for (k, l) in lx.iter().enumerate().skip(i) {
        if let Lx::Cs(a, b) = *l {
            match &text[a..b] {
                "\\begin" if group_name(k) == Some(name) => depth += 1,
                "\\end" if group_name(k) == Some(name) => {
                    if depth == 0 {
                        return Some(k);
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
    }
    None
}

/// The white space a token ending at `p` (content end `cend`) takes after
/// it: spaces, and a line end unless the next line is blank
/// (`line_start`: the token ended its line, so a line end is a blank line).
fn ws_after(lx: &[Lx], p: Pos, cend: usize, line_start: bool) -> (Pos, usize) {
    if p.sub > 0 {
        return (p, cend);
    }
    let mut i = p.i;
    let mut end = cend;
    let mut nl = line_start;
    while i < lx.len() {
        match lx[i] {
            Lx::Space(_, b) => {
                end = b;
                i += 1;
            }
            Lx::Nl(_, b) if !nl => {
                let blank = lx[i + 1..]
                    .iter()
                    .find(|l| !matches!(l, Lx::Space(..)))
                    .is_some_and(|l| matches!(l, Lx::Nl(..)));
                if blank {
                    break;
                }
                nl = true;
                end = b;
                i += 1;
            }
            _ => break,
        }
    }
    (Pos::at(i), end)
}

/// The white space from lexeme `i` of `lx` (not past `stop`; the content
/// begins at `at`): where it ends, and the first lexeme after it. A blank
/// line is not taken.
fn lead_ws(lx: &[Lx], i: usize, stop: usize, at: usize) -> (usize, usize) {
    let (p, end) = ws_after(&lx[..stop.min(lx.len())], Pos::at(i), at, false);
    (end, p.i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<(Kind, String)> {
        let sigs = Signatures::latexdiff();
        let r = Reader {
            text: src,
            sigs: &sigs,
        };
        r.seq(&lexemes(src, 0, src.len()))
            .into_iter()
            .map(|t| (t.kind, src[t.start..t.end].to_owned()))
            .collect()
    }

    #[test]
    fn words_and_commands() {
        let k = kinds("Hello world, see \\cite{a} and $x+y$.\n\nNext \\emph{b} % c\n  end");
        let texts: Vec<&str> = k.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(
            texts,
            [
                "Hello ",
                "world",
                ", ",
                "see ",
                "\\cite{a} ",
                "and ",
                "$x+y$",
                ".",
                "\n\n",
                "Next ",
                "\\emph{b} ",
                "% c\n  ",
                "end"
            ]
        );
        assert_eq!(k[4].0, Kind::Mbox);
        assert_eq!(k[6].0, Kind::Math);
        assert_eq!(k[8].0, Kind::Par);
        assert!(matches!(k[10].0, Kind::Text { .. }));
        assert_eq!(k[11].0, Kind::Comment);
    }

    #[test]
    fn environments() {
        let src =
            "\\begin{itemize}\n\\item a\n\\end{itemize}\n\\begin{equation}\nx\n\\end{equation}\n";
        let k = kinds(src);
        assert!(matches!(k[0].0, Kind::Env { .. }));
        assert!(matches!(&k[1].0, Kind::Display { env, .. } if env == "equation"));
        let sigs = Signatures::latexdiff();
        let r = Reader {
            text: src,
            sigs: &sigs,
        };
        let t = r.seq(&lexemes(src, 0, src.len()));
        assert_eq!(&src[t[0].start..t[0].open_end], "\\begin{itemize}\n");
        assert_eq!(&src[t[0].close_start..t[0].end], "\\end{itemize}\n");
        assert_eq!(t[0].children.len(), 2);
    }

    #[test]
    fn tabular_rows() {
        let src = "\\begin{tabular}{ll}\n\\hline\na & b \\\\\nc & d \\\\ \\hline\n\\end{tabular}";
        let sigs = Signatures::latexdiff();
        let r = Reader {
            text: src,
            sigs: &sigs,
        };
        let t = r.seq(&lexemes(src, 0, src.len()));
        let rows: Vec<&str> = t[0].children.iter().map(|c| &src[c.start..c.end]).collect();
        assert_eq!(rows, ["\\hline\na & b \\\\\n", "c & d \\\\ ", "\\hline\n"]);
    }
}
