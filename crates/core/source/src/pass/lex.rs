//! Tokens under the regime in force, the brace pairs, and the catcode
//! hazard spans.
//!
//! The tokenizer is `phitex-source`'s (`token_at`); this
//! module only says which [`Catcodes`] each stretch of text is read under.
//! A regime change takes effect where TeX would take it at the time it
//! reads the text: in the text's own run, never inside a definition's body
//! (whose tokens were read before it runs), and only as a prediction inside
//! a brace group that might be an argument (whose tokens were read when the
//! argument was taken). A catcode is local to its group, so a change ends
//! where its group does.

use crate::syntax::{Catcodes, SyntaxKind, token_at, verbatim_run};

use super::{FileKind, Scope, Status};

/// What a token is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokKind {
    Cs,
    Open,
    Close,
    /// Letters and other characters (one token per run).
    Text,
    Space,
    Newline,
    Comment,
    /// `#` and its digit.
    Param,
    /// A verbatim run (`\verb|…|`'s, a verbatim environment's body).
    Verbatim,
}

/// A token: its kind, its bytes, and how sure its tokenization is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokKind,
    pub start: usize,
    pub end: usize,
    /// Read under expl3's regime (spaces ignored).
    pub expl: bool,
    /// Inside a definition's body (`\def`, `\newcommand`, ...).
    pub in_body: bool,
    /// [`Status::Known`] under the file's own regime, `Predicted` under a
    /// regime the text visibly changed to, `Pending` where a change was
    /// not syntactically readable.
    pub tz: Status,
}

/// A stretch of text whose tokenization depends on a catcode change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hazard {
    pub start: usize,
    pub end: usize,
    pub cause: &'static str,
    pub detail: String,
    /// `Predicted`: re-tokenized under the visible regime; `Pending`: not
    /// knowable without evaluation (read under the regime before it).
    pub status: Status,
    /// The token that began it.
    pub token: usize,
}

pub(crate) struct Lexed {
    pub tokens: Vec<Token>,
    pub hazards: Vec<Hazard>,
    /// Token indices of each `{` and its `}`.
    pub braces: Vec<(usize, Option<usize>)>,
    pub unmatched_closes: Vec<usize>,
}

/// What an open brace group is to the regime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    /// A definition's body: changes in it take effect when it runs.
    Body,
    /// After a control sequence or another group: maybe an argument, so a
    /// change inside may come after its tokens were read.
    MaybeArg,
    /// A simple group or a box's material: changes take effect.
    Simple,
}

struct Frame {
    cat: Catcodes,
    role: Role,
    pre_expl: Option<Catcodes>,
    special: u128,
    /// `open.len()` when the group opened.
    open_spans: usize,
    tok: usize,
    brace: usize,
}

/// A group written as commands (`\begingroup`): what its end restores.
struct Soft {
    depth: usize,
    cat: Catcodes,
    pre_expl: Option<Catcodes>,
    special: u128,
    open_spans: usize,
    /// Its end was read inside an argument's braces opened in it: it
    /// takes effect when the argument has been read (at its `}`).
    ended: bool,
}

/// The commands a box's or an alignment's material follows: their group
/// runs as it is read.
const BOXES: &[&str] = &[
    "hbox",
    "vbox",
    "vtop",
    "vcenter",
    "halign",
    "valign",
    "noalign",
    "begingroup",
    "bgroup",
];

/// How many brace groups after the definer at `i..end` are code read now
/// and run later (its name's group counted too, harmlessly).
fn bodies(text: &str, name: &str, end: usize) -> u8 {
    let b = text.as_bytes();
    let next = || {
        let mut j = end;
        while j < b.len() && matches!(b[j], b' ' | b'\t' | b'\n' | b'\r' | b'*') {
            j += 1;
        }
        b.get(j).copied()
    };
    match name {
        "def" | "gdef" | "edef" | "xdef" => 1,
        "newcommand" | "renewcommand" | "providecommand" | "DeclareRobustCommand" | "@namedef" => {
            if next() == Some(b'{') {
                2
            } else {
                1
            }
        }
        "newenvironment" | "renewenvironment" => 3,
        "NewDocumentCommand"
        | "RenewDocumentCommand"
        | "ProvideDocumentCommand"
        | "DeclareDocumentCommand"
        | "NewExpandableDocumentCommand"
        | "DeclareExpandableDocumentCommand" => {
            if next() == Some(b'{') {
                3
            } else {
                2
            }
        }
        "NewDocumentEnvironment" | "RenewDocumentEnvironment" | "DeclareDocumentEnvironment" => 4,
        _ if is_expl_definer(name) => {
            if name.ends_with(":cpn") || name.ends_with(":cpx") || name.ends_with(":cn") {
                2
            } else {
                1
            }
        }
        _ => 0,
    }
}

/// `\cs_new:Npn` and its family: the definers of expl3.
pub(crate) fn is_expl_definer(name: &str) -> bool {
    (name.starts_with("cs_new") || name.starts_with("cs_set") || name.starts_with("cs_gset"))
        && name.contains(':')
        && !name.contains("eq:")
}

/// The commands whose expansion changes catcodes (TeX's, the kernel's,
/// expl3's): in a body they make the macro a catcode changer.
pub(crate) fn changes_catcodes(name: &str) -> bool {
    matches!(
        name,
        "catcode"
            | "makeatletter"
            | "makeatother"
            | "dospecials"
            | "@sanitize"
            | "obeylines"
            | "obeyspaces"
            | "@vobeyspaces"
            | "@makeother"
            | "ExplSyntaxOn"
            | "ExplSyntaxOff"
            | "cctab_select:N"
            | "cctab_begin:N"
            | "cctab_end:"
            | "tex_catcode:D"
    ) || name.starts_with("char_set_catcode")
}

/// The commands whose own syntax this pass reads (they are not hazards of
/// unknown extent even though they change catcodes).
fn handled_here(name: &str) -> bool {
    changes_catcodes(name)
        || crate::syntax::VERBATIM_COMMANDS.contains(&name)
        || matches!(
            name,
            "begin" | "verbatim" | "ProvidesExplPackage" | "ProvidesExplClass" | "ProvidesExplFile"
        )
}

/// A character code written after `` ` ``: `` `\@ ``, `` `@ ``, `` `\^^M ``.
fn char_code(b: &[u8], j: usize) -> Option<(u8, usize)> {
    let mut j = j;
    if b.get(j) == Some(&b'\\') {
        j += 1;
    }
    let c = *b.get(j)?;
    if c == b'^' && b.get(j + 1) == Some(&b'^') {
        let x = *b.get(j + 2)?;
        return Some((if x < 64 { x + 64 } else { x - 64 }, j + 3));
    }
    Some((c, j + 1))
}

fn skip_sp(b: &[u8], mut j: usize) -> usize {
    while j < b.len() && matches!(b[j], b' ' | b'\t') {
        j += 1;
    }
    j
}

/// `\catcode` from `j` (after its name), when its operands are literal:
/// the character and, if it is literal too, the new catcode.
pub(crate) fn catcode_assignment(b: &[u8], j: usize) -> Option<(u8, Option<u32>)> {
    let mut j = skip_sp(b, j);
    if b[j..].starts_with(b"\\string") {
        j = skip_sp(b, j + 7);
    }
    let ch = match b.get(j)? {
        b'`' => {
            let (c, k) = char_code(b, j + 1)?;
            j = k;
            c
        }
        c if c.is_ascii_digit() => {
            let s = j;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            u8::try_from(std::str::from_utf8(&b[s..j]).ok()?.parse::<u32>().ok()?).ok()?
        }
        b'"' => {
            let s = j + 1;
            j = s;
            while j < b.len() && matches!(b[j], b'0'..=b'9' | b'A'..=b'F') {
                j += 1;
            }
            u8::from_str_radix(std::str::from_utf8(&b[s..j]).ok()?, 16).ok()?
        }
        _ => return None,
    };
    j = skip_sp(b, j);
    if b.get(j) == Some(&b'=') {
        j = skip_sp(b, j + 1);
    }
    if b.get(j).is_some_and(u8::is_ascii_digit) {
        let s = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        return Some((ch, std::str::from_utf8(&b[s..j]).ok()?.parse().ok()));
    }
    if b.get(j) == Some(&b'\\') {
        let s = j + 1;
        let mut e = s;
        while e < b.len() && (b[e].is_ascii_alphabetic() || b[e] == b'@') {
            e += 1;
        }
        return Some((ch, named_value(&b[s..e])));
    }
    Some((ch, None))
}

/// The value of a constant the kernel names (`\active`, `\z@`, ...).
pub(crate) fn named_value(name: &[u8]) -> Option<u32> {
    Some(match name {
        b"active" => 13,
        b"other" => 12,
        b"letter" => 11,
        b"z@" => 0,
        b"@ne" => 1,
        b"tw@" => 2,
        b"thr@@" => 3,
        b"sixt@@n" => 16,
        _ => return None,
    })
}

/// Past the `*`, `[…]` and `{…}` that follow `j`, and at least to the
/// end of the line.
fn args_end(b: &[u8], mut j: usize) -> usize {
    loop {
        j = skip_sp(b, j);
        match b.get(j) {
            Some(b'*') => j += 1,
            Some(b'[') => match b[j..].iter().position(|&c| c == b']') {
                Some(k) => j += k + 1,
                None => return b.len(),
            },
            Some(b'{') => {
                let mut depth = 0usize;
                let mut k = j;
                while k < b.len() {
                    match b[k] {
                        b'{' => depth += 1,
                        b'}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    k += 1;
                }
                j = (k + 1).min(b.len());
            }
            _ => break,
        }
    }
    b[j..]
        .iter()
        .position(|&c| c == b'\n')
        .map_or(b.len(), |k| j + k)
}

/// The catcode the tokenizer reads `c` with under `cat`.
fn assumed(c: u8, cat: &Catcodes) -> u32 {
    match c {
        c if cat.is_open(c) => 1,
        c if cat.is_close(c) => 2,
        c if cat.is_escape(c) => 0,
        c if cat.is_other(c) && cat.is_letter(c) => 11,
        c if cat.is_other(c) => 12,
        b'\\' => 0,
        b'{' => 1,
        b'}' => 2,
        b'$' => 3,
        b'&' => 4,
        b'\r' | b'\n' => 5,
        b'#' => 6,
        b'^' => 7,
        b'_' if !cat.is_letter(b'_') => 8,
        b' ' | b'\t' => 10,
        b'%' => 14,
        b'~' => 13,
        0 => 9,
        127 => 15,
        c if cat.is_letter(c) => 11,
        _ => 12,
    }
}

/// `cat` with `c` back to its usual catcode as far as this regime goes
/// (an other `\\ { } % #`, a letter `{`, a group's character no more).
fn restored(cat: Catcodes, c: u8) -> Catcodes {
    let r = cat.with_other(c, false).with_shaping(c, 16);
    if structural(c) {
        r.without_letter(c)
    } else {
        r
    }
}

/// The characters whose catcode shapes tokens and groups.
pub(crate) fn structural(c: u8) -> bool {
    matches!(
        c,
        b'{' | b'}' | b'\\' | b'%' | b'#' | b'\n' | b'\r' | b' ' | b'\t'
    )
}

struct State<'s> {
    text: &'s str,
    tokens: Vec<Token>,
    hazards: Vec<Hazard>,
    /// Open hazard spans, innermost last.
    open: Vec<usize>,
    tz: Status,
    /// Characters whose catcode the text changed to one that does not
    /// shape groups (active, ignored, comment, ...): where they occur,
    /// tokenization is pending.
    special: u128,
    /// The same, for characters the document makes active somewhere.
    maybe_active: u128,
    /// Spans that end at a byte: (byte, hazard).
    timed: Vec<(usize, usize)>,
}

impl State<'_> {
    fn begin(&mut self, start: usize, cause: &'static str, detail: String, status: Status) {
        self.hazards.push(Hazard {
            start,
            end: start,
            cause,
            detail,
            status,
            token: self.tokens.len(),
        });
        self.open.push(self.hazards.len() - 1);
        self.tz = self.tz.max(status);
    }

    /// Ends the open spans from `from` on at `at`.
    fn end_from(&mut self, from: usize, at: usize) {
        for h in self.open.drain(from.min(self.open.len())..) {
            self.hazards[h].end = at;
        }
        self.retz();
    }

    /// Ends the innermost open span `cause` began (at `from` or after).
    fn end_one(&mut self, from: usize, cause: &str, at: usize) {
        if let Some(k) = (from..self.open.len())
            .rev()
            .find(|&k| self.hazards[self.open[k]].cause == cause)
        {
            let h = self.open.remove(k);
            self.hazards[h].end = at;
            self.retz();
        }
    }

    fn retz(&mut self) {
        self.tz = self
            .open
            .iter()
            .map(|&h| self.hazards[h].status)
            .max()
            .unwrap_or(Status::Known);
    }

    /// Whether a name being defined comes next (`\def\x`, `\let\x`,
    /// `\newcommand{\x}`): it is not called here.
    fn names_follow(&self) -> bool {
        let mut sig =
            self.tokens.iter().rev().filter(|t| {
                !matches!(t.kind, TokKind::Space | TokKind::Newline | TokKind::Comment)
            });
        let mut prev = sig.next();
        if prev.is_some_and(|t| t.kind == TokKind::Open) {
            prev = sig.next();
        }
        prev.is_some_and(|t| {
            t.kind == TokKind::Cs && {
                let n = &self.text[t.start + 1..t.end];
                bodies(self.text, n, t.end) > 0
                    || matches!(
                        n,
                        "let"
                            | "futurelet"
                            | "newif"
                            | "show"
                            | "meaning"
                            | "string"
                            | "noexpand"
                            | "MakeRobust"
                            | "@onlypreamble"
                            | "ifx"
                            | "ifdefined"
                            | "g@addto@macro"
                            | "addto@hook"
                            | "@ifdefinable"
                    )
                    || n.rsplit_once(':')
                        .is_some_and(|(_, sig)| sig.starts_with('N'))
            }
        })
    }

    /// Whether the token before is `=`: what follows is a value read, not
    /// a command run (`\catcode`\^^A=\catcode`\%`).
    fn after_equals(&self) -> bool {
        self.prev_sig()
            .is_some_and(|t| t.kind == TokKind::Text && self.text[t.start..t.end].ends_with('='))
    }

    fn prev_sig(&self) -> Option<&Token> {
        self.tokens
            .iter()
            .rev()
            .find(|t| !matches!(t.kind, TokKind::Space | TokKind::Newline | TokKind::Comment))
    }
}

/// Whether the byte at `i` is the character of an alphabetic constant
/// (`` \catcode`{ ``, ``=`}``): TeX reads it as a token only for its
/// code, §442, so a brace there opens or closes no group.
fn char_constant_at(st: &State<'_>, i: usize) -> bool {
    let b = st.text.as_bytes();
    if i == 0 || b[i - 1] != b'`' || b[i] == b'\\' {
        return false;
    }
    let Some(t) = st
        .tokens
        .last()
        .filter(|t| t.kind == TokKind::Text && t.end == i)
    else {
        return false;
    };
    let txt = &st.text[t.start..t.end];
    txt.ends_with("=`")
        || txt == "`"
            && st.tokens[..st.tokens.len() - 1]
                .iter()
                .rev()
                .find(|t| t.kind != TokKind::Space)
                .is_some_and(|c| {
                    c.kind == TokKind::Cs
                        && matches!(
                            &st.text[c.start + 1..c.end],
                            "catcode"
                                | "lccode"
                                | "uccode"
                                | "sfcode"
                                | "mathcode"
                                | "delcode"
                                | "char"
                        )
                })
}

/// Reads `text` under `kind`'s regime and the changes visible in it.
/// `scope` gives the macros and environments known to change catcodes
/// (none when harvesting definitions).
#[allow(clippy::too_many_lines)]
pub(crate) fn lex(text: &str, kind: FileKind, scope: Option<Scope<'_>>) -> Lexed {
    let b = text.as_bytes();
    let mut st = State {
        text,
        tokens: Vec::with_capacity(b.len() / 3 + 4),
        hazards: Vec::new(),
        open: Vec::new(),
        tz: Status::Known,
        special: 0,
        maybe_active: scope.map_or(0, |s| s.document.maybe_active()),
        timed: Vec::new(),
    };
    let mut braces: Vec<(usize, Option<usize>)> = Vec::new();
    let mut unmatched_closes = Vec::new();
    let mut frames: Vec<Frame> = Vec::new();
    let mut soft: Vec<Soft> = Vec::new();
    let mut cat = kind.catcodes();
    let mut role = Role::Simple;
    let mut body_depth = 0usize;
    let mut pre_expl: Option<Catcodes> = None;
    // (brace groups ahead that are bodies, at the depth where they come)
    let mut bodies_ahead: (usize, u8) = (0, 0);
    let mut verb: Option<std::ops::Range<usize>> = None;
    let mut i = 0;
    while i < b.len() {
        if let Some(v) = verb.clone().filter(|v| v.start <= i) {
            verb = None;
            if v.end > i {
                st.begin(i, "verbatim", String::new(), Status::Predicted);
                let k = st.open.len() - 1;
                st.tokens.push(Token {
                    kind: TokKind::Verbatim,
                    start: i,
                    end: v.end,
                    expl: cat.ignore_spaces,
                    in_body: body_depth > 0,
                    tz: st.tz,
                });
                st.end_from(k, v.end);
                i = v.end;
            }
            continue;
        }
        while let Some(k) = st.timed.iter().position(|&(e, _)| e <= i) {
            let (e, h) = st.timed.remove(k);
            if let Some(o) = st.open.iter().position(|&x| x == h) {
                st.open.remove(o);
                st.hazards[h].end = e;
                st.retz();
            }
        }
        let limit = verb.as_ref().map_or(b.len(), |v| v.start);
        let (sk, mut end) = if char_constant_at(&st, i) {
            // (`` `{ ``: the character's code, a number, §442; never a group)
            (SyntaxKind::Text, i + 1)
        } else {
            token_at(text, i, limit, &cat)
        };
        let mask = st.special | st.maybe_active;
        if sk == SyntaxKind::Text
            && mask != 0
            && body_depth == 0
            && let Some(k) = (i..end).find(|&k| b[k] < 128 && mask >> b[k] & 1 == 1)
        {
            if k > i {
                end = k;
            } else {
                // (a character the text or the document changed:
                // pending to its next occurrence on the line, a short
                // verbatim's extent, or to the line's end)
                let c = b[k];
                let le = b[k + 1..]
                    .iter()
                    .position(|&x| x == b'\n')
                    .map_or(b.len(), |p| k + 1 + p);
                let stop = b[k + 1..le]
                    .iter()
                    .position(|&x| x == c)
                    .map_or(le, |p| k + 2 + p);
                let why = if st.special >> c & 1 == 1 {
                    "character with changed catcode"
                } else {
                    "character active elsewhere in the document"
                };
                st.begin(i, why, (c as char).to_string(), Status::Pending);
                st.timed.push((stop, st.hazards.len() - 1));
                end = i + 1;
            }
        }
        let tk = match sk {
            SyntaxKind::Brace if b[i] == b'{' || cat.is_open(b[i]) => TokKind::Open,
            SyntaxKind::Brace | SyntaxKind::Other => TokKind::Close,
            SyntaxKind::Cs => TokKind::Cs,
            SyntaxKind::Space => TokKind::Space,
            SyntaxKind::Newline => TokKind::Newline,
            SyntaxKind::Comment => TokKind::Comment,
            SyntaxKind::Param => TokKind::Param,
            SyntaxKind::Verbatim => TokKind::Verbatim,
            _ => TokKind::Text,
        };
        if tk == TokKind::Comment && text[i..end].starts_with("%<@@=") && !cat.ignore_spaces {
            // (a docstrip `@@` module: the code is expl3's)
            pre_expl.get_or_insert(cat);
            cat = cat.expl();
            st.begin(i, "docstrip expl3 module", String::new(), Status::Predicted);
        }
        let here = st.tokens.len();
        let (tz0, cat0, in_body0) = (st.tz, cat, body_depth > 0);
        match tk {
            TokKind::Open => {
                let r = if bodies_ahead.1 > 0 && bodies_ahead.0 == frames.len() {
                    bodies_ahead.1 -= 1;
                    Role::Body
                } else if body_depth > 0 {
                    Role::Body
                } else {
                    match st.prev_sig() {
                        Some(t)
                            if t.kind == TokKind::Cs
                                && BOXES.contains(&&text[t.start + 1..t.end]) =>
                        {
                            Role::Simple
                        }
                        Some(t) if matches!(t.kind, TokKind::Cs | TokKind::Close) => Role::MaybeArg,
                        Some(t)
                            if t.kind == TokKind::Text && text[t.start..t.end].ends_with(']') =>
                        {
                            Role::MaybeArg
                        }
                        _ => Role::Simple,
                    }
                };
                frames.push(Frame {
                    cat,
                    role,
                    pre_expl,
                    special: st.special,
                    open_spans: st.open.len(),
                    tok: here,
                    brace: braces.len(),
                });
                braces.push((here, None));
                role = r;
                if r == Role::Body {
                    body_depth += 1;
                }
            }
            TokKind::Close => {
                if let Some(f) = frames.pop() {
                    if role == Role::Body {
                        body_depth -= 1;
                    }
                    braces[f.brace].1 = Some(here);
                    debug_assert_eq!(braces[f.brace].0, f.tok);
                    st.end_from(f.open_spans, i);
                    cat = f.cat;
                    st.special = f.special;
                    soft.retain(|g| g.depth <= frames.len());
                    role = f.role;
                    pre_expl = f.pre_expl;
                    if soft
                        .last()
                        .is_some_and(|g| g.ended && g.depth == frames.len())
                        && let Some(g) = soft.pop()
                    {
                        // (the argument holding the group's end has been
                        // read: the end runs before the text after it)
                        st.end_from(g.open_spans, i);
                        cat = g.cat;
                        pre_expl = g.pre_expl;
                        st.special = g.special;
                    }
                    if bodies_ahead.0 > frames.len() {
                        bodies_ahead = (0, 0);
                    }
                } else {
                    unmatched_closes.push(here);
                }
            }
            TokKind::Cs => {
                let name = &text[i + 1..end];
                if verb.is_none() {
                    verb = verbatim_run(text, i);
                }
                let n = bodies(text, name, end);
                if n > 0 {
                    bodies_ahead = (frames.len(), n);
                }
                if body_depth == 0 && matches!(name, "begingroup" | "bgroup" | "group_begin:") {
                    soft.push(Soft {
                        depth: frames.len(),
                        cat,
                        pre_expl,
                        special: st.special,
                        open_spans: st.open.len(),
                        ended: false,
                    });
                } else if body_depth == 0
                    && matches!(name, "endgroup" | "egroup" | "group_end:")
                    && let Some(g) = soft.last_mut().filter(|g| {
                        !g.ended
                            && g.depth < frames.len()
                            && frames[g.depth..]
                                .iter()
                                .skip(1)
                                .all(|f| f.role == Role::MaybeArg)
                            && role == Role::MaybeArg
                    })
                {
                    // (braces around an argument are not a group, §473: the
                    // end is the outer group's, run once the argument is read)
                    g.ended = true;
                } else if body_depth == 0
                    && matches!(name, "endgroup" | "egroup" | "group_end:")
                    && soft.last().is_some_and(|f| f.depth == frames.len())
                    && let Some(f) = soft.pop()
                {
                    // (a group written as commands ends the catcode changes
                    // made in it, as a brace group does)
                    st.end_from(f.open_spans, i);
                    cat = f.cat;
                    pre_expl = f.pre_expl;
                    st.special = f.special;
                } else if body_depth == 0
                    && !st.names_follow()
                    && !st.after_equals()
                    && soft.last().is_some_and(|f| f.depth == frames.len())
                    && scope.is_some_and(|s| {
                        if s.document.defines(name) {
                            s.document.ends_group(name, Some((s.file, i)))
                        } else {
                            s.prelude.ends_group(name, None)
                        }
                    })
                    && let Some(f) = soft.pop()
                {
                    // (a macro running the `\endgroup` of a group begun
                    // here: its catcode changes end, §1063)
                    st.end_from(f.open_spans, i);
                    cat = f.cat;
                    pre_expl = f.pre_expl;
                    st.special = f.special;
                } else if body_depth == 0 && !st.names_follow() && !st.after_equals() {
                    let floor = frames.last().map_or(0, |f| f.open_spans).max(
                        soft.last()
                            .filter(|f| f.depth == frames.len())
                            .map_or(0, |f| f.open_spans),
                    );
                    // (a change in a group that may be an argument: its
                    // tokens may have been read already)
                    let sure = if role == Role::MaybeArg {
                        Status::Pending
                    } else {
                        Status::Predicted
                    };
                    switch(
                        &mut st,
                        &mut cat,
                        &mut pre_expl,
                        name,
                        i,
                        end,
                        floor,
                        sure,
                        scope,
                    );
                }
            }
            _ => {}
        }
        st.tokens.push(Token {
            kind: tk,
            start: i,
            end,
            expl: cat0.ignore_spaces,
            in_body: in_body0,
            tz: tz0,
        });
        i = end;
    }
    st.end_from(0, b.len());
    Lexed {
        tokens: st.tokens,
        hazards: st.hazards,
        braces,
        unmatched_closes,
    }
}

/// The regime change the control sequence `name` (bytes `i..end`) makes, if
/// it makes one visible in the text.
#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::match_same_arms
)]
fn switch(
    st: &mut State<'_>,
    cat: &mut Catcodes,
    pre_expl: &mut Option<Catcodes>,
    name: &str,
    i: usize,
    end: usize,
    floor: usize,
    sure: Status,
    scope: Option<Scope<'_>>,
) {
    let b = st.text.as_bytes();
    let pending = |st: &mut State<'_>, cause, detail: &str| {
        st.begin(i, cause, detail.to_owned(), Status::Pending);
    };
    match name {
        "makeatletter" => {
            *cat = cat.with_letter(b'@');
            st.begin(i, "\\makeatletter", String::new(), sure);
        }
        "makeatother" => {
            *cat = cat.without_letter(b'@');
            st.end_one(floor, "\\makeatletter", i);
        }
        "ExplSyntaxOn" => {
            pre_expl.get_or_insert(*cat);
            *cat = cat.expl();
            st.begin(i, "\\ExplSyntaxOn", String::new(), sure);
        }
        "ExplSyntaxOff" => {
            if let Some(c) = pre_expl.take() {
                *cat = c;
            }
            st.end_one(floor, "\\ExplSyntaxOn", i);
        }
        "ProvidesExplPackage" | "ProvidesExplClass" | "ProvidesExplFile" => {
            *cat = cat.expl();
            st.begin(i, "\\ProvidesExpl…", String::new(), sure);
        }
        "catcode" | "tex_catcode:D" => match catcode_assignment(b, end) {
            Some((c, v)) => assign(st, cat, c, v, i, floor, sure),
            None => st.begin(
                i,
                "\\catcode with computed operands",
                String::new(),
                Status::Pending,
            ),
        },
        "@makeother" | "char_set_catcode_other:N" | "char_set_catcode_letter:N" => {
            // (`\catcode` to 12, or 11, by another name)
            let v = if name.ends_with("letter:N") { 11 } else { 12 };
            match char_code(b, skip_sp(b, end)) {
                Some((c, _)) => assign(st, cat, c, Some(v), i, floor, sure),
                None => pending(st, "\\@makeother of a shaping character", ""),
            }
        }
        "dospecials" | "@sanitize" | "obeylines" | "obeyspaces" | "@vobeyspaces"
        | "cctab_select:N" | "cctab_begin:N" => pending(st, "catcode regime switch", name),
        _ if name.starts_with("char_set_catcode") => {
            // (`:N \X` or `:n { `\X }`: a literal character)
            let mut j = skip_sp(b, end);
            if name.ends_with(":n") && b.get(j) == Some(&b'{') {
                j = skip_sp(b, j + 1);
                if b.get(j) == Some(&b'`') {
                    j += 1;
                }
            }
            match char_code(b, j) {
                Some((c, _)) if c >= 128 => {}
                Some((c, _)) if !structural(c) => st.special |= 1 << c,
                _ => pending(st, "catcode regime switch", name),
            }
        }
        "begin" => {
            if let Some(s) = scope {
                let j = skip_sp(b, end);
                if b.get(j) == Some(&b'{')
                    && let Some(k) = b[j + 1..].iter().take(64).position(|&c| c == b'}')
                {
                    let env = &st.text[j + 1..j + 1 + k];
                    if s.document.env_changes_catcodes(env) || s.prelude.env_changes_catcodes(env) {
                        // (to the environment's end, found as text)
                        let pat = format!("\\end{{{env}}}");
                        let stop = st.text[end..].find(&pat).map_or(b.len(), |p| end + p);
                        st.begin(
                            i,
                            "environment changing catcodes",
                            env.to_owned(),
                            Status::Pending,
                        );
                        st.timed.push((stop, st.hazards.len() - 1));
                    }
                }
            }
        }
        _ => {
            if let Some(s) = scope
                && !handled_here(name)
            {
                let k = if s.document.defines(name) {
                    s.document.changes_catcodes(name, Some((s.file, i)))
                } else {
                    s.prelude.changes_catcodes(name, None)
                };
                let (k, chars) = k;
                let of_args = if s.document.defines(name) {
                    s.document.arg_catcodes(name, Some((s.file, i)))
                } else {
                    s.prelude.arg_catcodes(name, None)
                };
                if matches!(k, 0 | 3) && !of_args.is_empty() {
                    // (the catcodes of the characters its arguments name,
                    // §442: read here, as it reads them)
                    let n = of_args.iter().map(|&(p, _)| p).max().unwrap_or(0);
                    let named = call_chars(b, end, n, cat);
                    for &(p, v) in of_args {
                        match named.get(usize::from(p) - 1).copied().flatten() {
                            Some(c) => assign(st, cat, c, Some(v), i, floor, sure),
                            None => pending(st, "macro changing catcodes of its arguments", name),
                        }
                    }
                }
                if k == 3 {
                    // (the characters set to a literal catcode: as a
                    // `\catcode` here would, §1232; the others pending
                    // where they occur)
                    let set = if s.document.defines(name) {
                        s.document.set_catcodes(name, Some((s.file, i)))
                    } else {
                        s.prelude.set_catcodes(name, None)
                    };
                    let mut known = 0u128;
                    for &(c, v) in set {
                        assign(st, cat, c, Some(v), i, floor, sure);
                        known |= 1 << c;
                    }
                    st.special |= chars & !known;
                } else if k == 2 {
                    pending(st, "macro changing catcodes", name);
                } else if k == 1 {
                    // (the change lasts while it reads its arguments: to the
                    // end of those that follow on its line, or the line)
                    st.begin(
                        i,
                        "macro changing catcodes for its arguments",
                        name.to_owned(),
                        Status::Pending,
                    );
                    st.timed.push((args_end(b, end), st.hazards.len() - 1));
                }
            }
        }
    }
}

/// The characters the `n` undelimited arguments from `j` name, each a
/// control symbol, a one-letter control word, a character, or one of these
/// braced (`None`: another argument).
fn call_chars(b: &[u8], mut j: usize, n: u8, cat: &Catcodes) -> Vec<Option<u8>> {
    let letter = |c: u8| c.is_ascii_alphabetic() || cat.is_letter(c);
    let mut out = Vec::new();
    for _ in 0..n {
        // (spaces, an end of line and comments before an argument are
        // skipped, §441, §347)
        loop {
            j = skip_sp(b, j);
            match b.get(j) {
                Some(b'%') => {
                    j = b[j..]
                        .iter()
                        .position(|&c| c == b'\n')
                        .map_or(b.len(), |k| j + k + 1);
                }
                Some(b'\n') if b.get(skip_sp(b, j + 1)) != Some(&b'\n') => j += 1,
                _ => break,
            }
        }
        let one = |j: usize| -> (Option<u8>, usize) {
            match b.get(j) {
                Some(b'\\') => match char_code(b, j) {
                    Some((c, k)) if b[j + 1] == b'^' || !letter(c) => (Some(c), k),
                    Some((c, k)) => {
                        let e = (k..b.len()).find(|&e| !letter(b[e])).unwrap_or(b.len());
                        (if e == k { Some(c) } else { None }, e)
                    }
                    None => (None, b.len()),
                },
                Some(&c) if c != b'{' && c != b'}' => (Some(c), j + 1),
                _ => (None, j + 1),
            }
        };
        if b.get(j) == Some(&b'{') {
            let (c, k) = one(skip_sp(b, j + 1));
            let k = skip_sp(b, k);
            if b.get(k) == Some(&b'}') {
                out.push(c);
                j = k + 1;
            } else {
                out.push(None);
                break;
            }
        } else {
            let (c, k) = one(j);
            out.push(c);
            j = k;
        }
    }
    out
}

/// `\catcode` of the character `c` set to `v` (`None`: a value not
/// literal), §1232: the regime from `i` on.
fn assign(
    st: &mut State<'_>,
    cat: &mut Catcodes,
    c: u8,
    v: Option<u32>,
    i: usize,
    floor: usize,
    sure: Status,
) {
    match (c, v) {
        (c, Some(v)) if assumed(c, &restored(*cat, c)) == v => {
            *cat = restored(*cat, c);
            let d = format!("{}=", c as char);
            if let Some(k) = (floor..st.open.len()).rev().find(|&k| {
                let h = &st.hazards[st.open[k]];
                matches!(
                    h.cause,
                    "\\catcode to a shaping class" | "\\catcode literal"
                ) && h.detail.starts_with(&d)
            }) {
                let h = st.open.remove(k);
                st.hazards[h].end = i;
                st.retz();
            }
        }
        (c, Some(v @ 0..=2)) if !structural(c) && c < 128 => {
            *cat = cat.with_shaping(c, v);
            st.begin(i, "\\catcode literal", format!("{}={v}", c as char), sure);
        }
        (c, Some(_)) if cat.is_shaping(c) => {
            // (no longer an escape or a group's: then as from its usual
            // catcode)
            *cat = cat.with_shaping(c, 16);
            assign(st, cat, c, v, i, floor, sure);
        }
        (c @ (b'{' | b'}'), Some(11)) => {
            *cat = cat.with_other(c, true).with_letter(c);
            st.begin(i, "\\catcode literal", format!("{}=11", c as char), sure);
        }
        (c, Some(11)) if !structural(c) => {
            *cat = cat.with_letter(c);
            st.begin(i, "\\catcode literal", format!("{}=11", c as char), sure);
        }
        (c, Some(12)) if matches!(c, b'\\' | b'{' | b'}' | b'%' | b'#') => {
            *cat = cat.with_other(c, true);
            st.begin(i, "\\catcode literal", format!("{}=12", c as char), sure);
        }
        (c, Some(12)) if !structural(c) => {
            *cat = cat.without_letter(c);
            st.begin(i, "\\catcode literal", format!("{}=12", c as char), sure);
        }
        (c, Some(v)) if !structural(c) && matches!(v, 3 | 4 | 7 | 8) => {}
        (b'~', Some(13)) => {}
        // (a byte past ASCII: UTF-8 text, which the kernel makes active
        // anyway; the tokens here do not depend on it)
        (c, _) if c >= 128 => {}
        (c, _) if !structural(c) => {
            *cat = cat.without_letter(c);
            st.special |= 1 << c;
        }
        (c, Some(v)) => st.begin(
            i,
            "\\catcode to a shaping class",
            format!("{}={v}", c as char),
            Status::Pending,
        ),
        (_, None) => st.begin(
            i,
            "\\catcode with computed operands",
            String::new(),
            Status::Pending,
        ),
    }
}
