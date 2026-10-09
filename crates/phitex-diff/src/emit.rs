//! The markup: the new body with the old one's differences marked as
//! latexdiff marks them, and the change list.

use crate::myers::patience as lcs;
use crate::sig::{Class, EnvKind, Signatures};
use crate::tok::{Kind, SECTIONS, Tok};

/// Common runs shorter than this between two changes are made part of
/// them (latexdiff's `MINWORDSBLOCK`).
const MIN_BLOCK: usize = 3;

/// Which side a run is marked as.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Add,
    Del,
}

/// The context of a run: in a float or an alignment (`fl`: the `FL`
/// markup); in a list (`list`: a deleted item is shown as an item, its
/// text struck out, as latexdiff shows it; in a numbered list or a
/// bibliography as `\item[]`, which takes no number).
#[derive(Clone, Copy, Default)]
struct Ctx {
    fl: bool,
    list: List,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum List {
    #[default]
    None,
    Bullets,
    Numbered,
}

impl Ctx {
    /// The context inside node `t`.
    fn enter(self, t: &Tok) -> Ctx {
        match &t.kind {
            Kind::Env { name, kind } => Ctx {
                fl: self.fl || matches!(kind, EnvKind::Float | EnvKind::Tabular),
                list: match (kind, name.as_str()) {
                    (EnvKind::List, "itemize" | "description") => List::Bullets,
                    (EnvKind::List, _) => List::Numbered,
                    _ => List::None,
                },
            },
            _ => self,
        }
    }
}

/// A change, in flat offsets.
#[derive(Clone, Debug)]
pub struct RawChange {
    pub old: (usize, usize),
    pub new: (usize, usize),
    pub out: (usize, usize),
    pub section: Option<String>,
}

/// The markup's writer.
pub struct Em<'a> {
    pub old: &'a str,
    pub new: &'a str,
    pub sigs: &'a Signatures,
    /// The new preamble loads amsmath (`align*` is there for deleted
    /// alignments).
    pub amsmath: bool,
    pub out: String,
    pub changes: Vec<RawChange>,
    /// The section the text written is in (the title of the last
    /// sectioning command written).
    pub section: Option<String>,
    /// A line end is owed before the next text (a comment was written
    /// last): unless that text begins with one.
    pub nl: bool,
}

impl<'a> Em<'a> {
    #[must_use]
    pub fn new(old: &'a str, new: &'a str, sigs: &'a Signatures, amsmath: bool) -> Em<'a> {
        Em {
            old,
            new,
            sigs,
            amsmath,
            out: String::new(),
            changes: Vec::new(),
            section: None,
            nl: false,
        }
    }

    /// Write `s`, after the line end a comment is owed.
    pub fn push(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        if self.nl && !s.starts_with('\n') {
            self.out.push('\n');
        }
        self.nl = false;
        self.out.push_str(s);
    }

    /// The output, the line end owed written.
    pub fn finish(mut self) -> (String, Vec<RawChange>) {
        if self.nl {
            self.out.push('\n');
        }
        (self.out, self.changes)
    }

    /// The body: paragraphs aligned first (a blank line ends one), then
    /// each run of unmatched ones diffed by tokens. `old_end`, `new_end`:
    /// where the bodies end.
    pub fn body(&mut self, old: &[Tok], new: &[Tok], old_end: usize, new_end: usize) {
        let oc = chunks(old);
        let nc = chunks(new);
        let ok: Vec<u64> = oc.iter().map(|r| chunk_key(&old[r.clone()])).collect();
        let nk: Vec<u64> = nc.iter().map(|r| chunk_key(&new[r.clone()])).collect();
        let pairs = lcs(&ok, &nk);
        let (mut i, mut j) = (0, 0);
        for (pi, pj) in pairs
            .into_iter()
            .chain(std::iter::once((oc.len(), nc.len())))
        {
            if i < pi || j < pj {
                let oa = oc.get(i).map_or(old.len(), |r| r.start);
                let ob = oc.get(pi).map_or(old.len(), |r| r.start);
                let na = nc.get(j).map_or(new.len(), |r| r.start);
                let nb = nc.get(pj).map_or(new.len(), |r| r.start);
                let oe = old.get(ob).map_or(old_end, |t| t.start);
                let ne = new.get(nb).map_or(new_end, |t| t.start);
                self.run(&old[oa..ob], &new[na..nb], oe, ne);
            }
            if pi < oc.len() {
                for t in &new[nc[pj].clone()] {
                    self.same(t);
                }
            }
            i = pi + 1;
            j = pj + 1;
        }
    }

    /// A run of paragraphs that differ, diffed by tokens (`old_end`,
    /// `new_end`: where the tokens after them begin).
    pub fn run(&mut self, old: &[Tok], new: &[Tok], old_end: usize, new_end: usize) {
        self.seq(old, new, old_end, new_end, Ctx::default());
    }

    /// Diff two token sequences (ending at `old_end`, `new_end`).
    fn seq(&mut self, old: &[Tok], new: &[Tok], old_end: usize, new_end: usize, ctx: Ctx) {
        let ok: Vec<u64> = old.iter().map(|t| t.key).collect();
        let nk: Vec<u64> = new.iter().map(|t| t.key).collect();
        let pairs = merge_short(&lcs(&ok, &nk), old, new);
        let (mut i, mut j) = (0, 0);
        for (pi, pj) in pairs
            .into_iter()
            .chain(std::iter::once((old.len(), new.len())))
        {
            if i < pi || j < pj {
                let oe = old.get(pi).map_or(old_end, |t| t.start);
                let ne = new.get(pj).map_or(new_end, |t| t.start);
                self.hunk(&old[i..pi], &new[j..pj], oe, ne, ctx);
            }
            if pi < old.len() {
                self.same(&new[pj]);
            }
            i = pi + 1;
            j = pj + 1;
        }
    }

    /// A run of changes: nodes with the same prefix (a command whose text
    /// changed, an environment whose body did) are paired and diffed
    /// inside; the rest is deleted and added.
    fn hunk(&mut self, old: &[Tok], new: &[Tok], old_end: usize, new_end: usize, ctx: Ctx) {
        // (only nodes pair: the leaves were not common, or the first diff
        // would have matched them)
        let key = |t: &Tok, side: usize, i: usize| {
            if t.is_node() {
                (0, t.prefix)
            } else {
                (side, i as u64)
            }
        };
        let ok: Vec<_> = old.iter().enumerate().map(|(i, t)| key(t, 1, i)).collect();
        let nk: Vec<_> = new.iter().enumerate().map(|(i, t)| key(t, 2, i)).collect();
        let pairs = lcs(&ok, &nk);
        let (mut i, mut j) = (0, 0);
        for (pi, pj) in pairs
            .into_iter()
            .chain(std::iter::once((old.len(), new.len())))
        {
            if i < pi || j < pj {
                let oe = old.get(pi).map_or(old_end, |t| t.start);
                let ne = new.get(pj).map_or(new_end, |t| t.start);
                self.block(&old[i..pi], &new[j..pj], oe, ne, ctx);
            }
            if pi < old.len() {
                let (o, n) = (&old[pi], &new[pj]);
                if o.key == n.key || !n.is_node() {
                    self.same(n);
                } else {
                    self.pair(o, n, ctx);
                }
            }
            i = pi + 1;
            j = pj + 1;
        }
    }

    /// Two nodes with the same prefix: the new one's open and close parts,
    /// and their children diffed.
    fn pair(&mut self, o: &Tok, n: &Tok, ctx: Ctx) {
        self.note_section(n);
        let ctx = ctx.enter(n);
        self.push(&self.new[n.start..n.open_end]);
        self.seq(&o.children, &n.children, o.close_start, n.close_start, ctx);
        self.push(&self.new[n.close_start..n.end]);
    }

    /// A token the same on both sides.
    fn same(&mut self, n: &Tok) {
        self.note_section(n);
        self.push(&self.new[n.start..n.end]);
    }

    fn note_section(&mut self, n: &Tok) {
        if let Kind::Text { name, .. } = &n.kind
            && SECTIONS.contains(&name.as_str())
        {
            self.section = Some(self.new[n.open_end..n.close_start].trim().to_owned());
        }
    }

    /// Deleted tokens `old` and added `new` (where they are when empty:
    /// `old_at`, `new_at`): one change.
    fn block(&mut self, old: &[Tok], new: &[Tok], old_at: usize, new_at: usize, ctx: Ctx) {
        let span = |t: &[Tok], at: usize| match (t.first(), t.last()) {
            (Some(a), Some(b)) => (a.start, b.cend),
            _ => (at, at),
        };
        let o = span(old, old_at);
        let n = span(new, new_at);
        let start = self.out.len();
        let fl = if ctx.fl { "FL" } else { "" };
        if !old.is_empty() {
            self.push(&format!("\\DIFdelbegin{fl} "));
            self.mark(old, Side::Del, ctx);
            self.push(&format!("\\DIFdelend{fl} "));
        }
        if !new.is_empty() {
            self.push(&format!("\\DIFaddbegin{fl} "));
            self.mark(new, Side::Add, ctx);
            self.push(&format!("\\DIFaddend{fl} "));
        }
        // (the section a change is in: the last one before it, or its own)
        let section = self.section.clone();
        self.changes.push(RawChange {
            old: o,
            new: n,
            out: (start, self.out.len()),
            section,
        });
    }

    fn text(&self, side: Side) -> &'a str {
        match side {
            Side::Add => self.new,
            Side::Del => self.old,
        }
    }

    /// Mark tokens as added or deleted: runs of those that can sit in the
    /// markup in `\DIFadd{…}`/`\DIFdel{…}`; the others outside (added) or
    /// commented out (deleted); nodes opened and marked inside.
    #[allow(clippy::too_many_lines)]
    fn mark(&mut self, toks: &[Tok], side: Side, ctx: Ctx) {
        let src = self.text(side);
        let mut buf = String::new();
        for t in toks {
            let text = &src[t.start..t.end];
            match &t.kind {
                Kind::Word | Kind::Safe | Kind::Math
                    if side == Side::Del && self.old_only(text) =>
                {
                    self.flush(&mut buf, side, ctx);
                    self.comment_out(text);
                }
                Kind::Word | Kind::Safe | Kind::Math | Kind::Ws => buf.push_str(text),
                Kind::Mbox => {
                    buf.push_str("\\mbox{");
                    buf.push_str(&src[t.start..t.cend]);
                    buf.push_str("}\\hskip0pt ");
                    buf.push_str(&src[t.cend..t.end]);
                }
                Kind::Comment => {
                    self.flush(&mut buf, side, ctx);
                    self.push(text);
                    self.nl = !text.ends_with('\n');
                }
                Kind::Unsafe
                | Kind::Rule
                | Kind::Picture
                | Kind::Par
                | Kind::Amp
                | Kind::RowEnd => {
                    self.flush(&mut buf, side, ctx);
                    match side {
                        // (an added item's label marked: `\item[\DIFadd{x}]`)
                        Side::Add => match item_label(text) {
                            Some((a, b)) => {
                                self.push(&text[..a]);
                                let mut label = text[a..b].to_owned();
                                self.flush(&mut label, side, ctx);
                                self.push(&text[b..]);
                            }
                            None => self.push(text),
                        },
                        Side::Del => {
                            self.comment_out(text);
                            // (a deleted item: an item still, its label
                            // struck out)
                            if ctx.list == List::Numbered && is_item(text) {
                                self.push("\\item[] ");
                            } else if ctx.list == List::Bullets && is_item(text) {
                                self.push("\\item");
                                if let Some((a, b)) = item_label(text) {
                                    self.push("[");
                                    let mut label = text[a..b].to_owned();
                                    self.flush(&mut label, side, ctx);
                                    self.push("]");
                                }
                                self.push(" ");
                            }
                        }
                    }
                }
                Kind::Display { env, arr, body } => {
                    self.flush(&mut buf, side, ctx);
                    self.display(t, env, *arr, *body, side);
                }
                Kind::Text { name, class, star } => {
                    self.flush(&mut buf, side, ctx);
                    if side == Side::Add {
                        self.note_section(t);
                    }
                    match (side, class) {
                        (Side::Del, _) if self.sigs.old_only(name) => self.comment_out(text),
                        (Side::Del, Class::Context2) => self.comment_out(text),
                        (Side::Del, Class::Context1) => {
                            self.mark(&t.children, side, ctx);
                            self.push(&src[t.cend..t.end]);
                        }
                        (Side::Del, Class::Text { counter: true }) if !star => {
                            self.push(&src[t.start..t.open_end]);
                            self.mark(&t.children, side, ctx);
                            self.push(&src[t.close_start..t.cend]);
                            self.push(&format!("\\addtocounter{{{name}}}{{-1}}"));
                            self.push(&src[t.cend..t.end]);
                        }
                        _ => self.node(t, side, ctx),
                    }
                }
                Kind::Group => {
                    self.flush(&mut buf, side, ctx);
                    self.node(t, side, ctx);
                }
                Kind::Row => {
                    self.flush(&mut buf, side, ctx);
                    self.row(t, side, ctx);
                }
                Kind::Env { name, kind } => {
                    self.flush(&mut buf, side, ctx);
                    let inner = ctx.enter(t);
                    match (side, kind) {
                        (Side::Add, _) => self.node(t, side, inner),
                        (Side::Del, EnvKind::Tabular)
                            if matches!(name.as_str(), "tabular" | "tabular*") =>
                        {
                            self.node(t, side, inner);
                        }
                        (Side::Del, EnvKind::Tabular) => self.comment_out(text),
                        (Side::Del, _) => {
                            // (its items are not items any more)
                            let inner = Ctx {
                                list: List::None,
                                ..inner
                            };
                            self.comment_out(&src[t.start..t.open_end]);
                            self.mark(&t.children, side, inner);
                            self.comment_out(&src[t.close_start..t.end]);
                        }
                    }
                }
            }
        }
        self.flush(&mut buf, side, ctx);
    }

    /// A node marked inside: its open and close parts as they are.
    fn node(&mut self, t: &Tok, side: Side, ctx: Ctx) {
        let src = self.text(side);
        self.push(&src[t.start..t.open_end]);
        self.mark(&t.children, side, ctx);
        self.push(&src[t.close_start..t.end]);
    }

    /// An alignment's row: a deleted one is shown, its cells struck out,
    /// its `&` and `\\` kept (its rules commented out).
    fn row(&mut self, t: &Tok, side: Side, ctx: Ctx) {
        if side == Side::Add {
            self.mark(&t.children, side, ctx);
            return;
        }
        let src = self.old;
        let mut run: Vec<Tok> = Vec::new();
        for k in &t.children {
            if matches!(k.kind, Kind::Amp | Kind::RowEnd) {
                self.mark(&run, side, ctx);
                run.clear();
                self.push(&src[k.start..k.end]);
            } else {
                run.push(k.clone());
            }
        }
        self.mark(&run, side, ctx);
    }

    /// Display math, coarse: an added one as it is, its body marked; a
    /// deleted one as an unnumbered display (`displaymath`, or `align*`,
    /// `eqnarray*` for an alignment) of its body struck out, its labels
    /// commented out.
    fn display(&mut self, t: &Tok, env: &str, arr: bool, body: (usize, usize), side: Side) {
        let src = self.text(side);
        match side {
            Side::Add => {
                self.push(&src[t.start..body.0]);
                self.math(&src[body.0..body.1], side, None);
                self.push(&src[body.1..t.end]);
            }
            Side::Del => {
                let (repl, max_amp) = if !arr {
                    ("displaymath", Some(0))
                } else if self.amsmath {
                    ("align*", None)
                } else {
                    ("eqnarray*", Some(2))
                };
                let _ = env;
                self.push(&format!("\\begin{{{repl}}}"));
                self.math(&src[body.0..body.1], side, max_amp);
                self.push(&format!("\\end{{{repl}}}"));
                self.push(&src[t.cend..t.end]);
            }
        }
    }

    /// A math body marked: each cell (between `&` and `\\`, outside nested
    /// environments) in `\DIFadd{…}`/`\DIFdel{…}`; labels, tags and
    /// `\intertext` outside (deleted: commented out); `max_amp`: how many
    /// `&` a row may keep (the replacement's columns).
    fn math(&mut self, body: &str, side: Side, max_amp: Option<usize>) {
        let b = body.as_bytes();
        let mut buf = String::new();
        let mut depth = 0usize;
        let mut amps = 0usize;
        let mut i = 0;
        let ctx = Ctx::default();
        while i < b.len() {
            match b[i] {
                b'%' => {
                    let e = line_end(b, i);
                    self.flush(&mut buf, side, ctx);
                    self.push(&body[i..e]);
                    i = e;
                }
                b'{' => {
                    let e = brace_end(b, i);
                    buf.push_str(&body[i..e]);
                    i = e;
                }
                b'$' if side == Side::Del => i += 1,
                b'&' if depth == 0 => {
                    self.flush(&mut buf, side, ctx);
                    amps += 1;
                    if max_amp.is_none_or(|m| amps <= m) {
                        self.push("&");
                    } else {
                        self.push(" ");
                    }
                    i += 1;
                }
                b'\\' => {
                    let (name, mut e) = cs_at(body, i);
                    match name {
                        // (math delimiters in math: an old version that
                        // does not compile; dropped from a deleted body)
                        "[" | "]" | "(" | ")" if side == Side::Del => {}
                        "begin" => {
                            depth += 1;
                            buf.push_str(&body[i..e]);
                        }
                        "end" => {
                            depth = depth.saturating_sub(1);
                            buf.push_str(&body[i..e]);
                        }
                        "\\" if depth == 0 => {
                            self.flush(&mut buf, side, ctx);
                            // (its star and optional argument)
                            if b.get(e) == Some(&b'*') {
                                e += 1;
                            }
                            if b.get(e) == Some(&b'[')
                                && let Some(k) = body[e..].find(']')
                            {
                                e += k + 1;
                            }
                            amps = 0;
                            if max_amp == Some(0) {
                                self.push(" ");
                            } else {
                                self.push(&body[i..e]);
                            }
                        }
                        "label" | "tag" | "nonumber" | "notag" | "qedhere" | "intertext"
                        | "shortintertext"
                            if depth == 0 =>
                        {
                            self.flush(&mut buf, side, ctx);
                            if b.get(e) == Some(&b'*') {
                                e += 1;
                            }
                            let mut a = e;
                            while a < b.len() && b[a] == b' ' {
                                a += 1;
                            }
                            if b.get(a) == Some(&b'{') {
                                e = brace_end(b, a);
                            }
                            match side {
                                Side::Add => self.push(&body[i..e]),
                                Side::Del => self.comment_out(&body[i..e]),
                            }
                        }
                        _ => buf.push_str(&body[i..e]),
                    }
                    i = e;
                }
                _ => {
                    let ch = body[i..].chars().next().map_or(1, char::len_utf8);
                    buf.push_str(&body[i..i + ch]);
                    i += ch;
                }
            }
        }
        self.flush(&mut buf, side, ctx);
    }

    /// Write the run in `buf` in its markup (white space alone as it is).
    fn flush(&mut self, buf: &mut String, side: Side, ctx: Ctx) {
        if buf.is_empty() {
            return;
        }
        if buf.trim().is_empty() {
            self.push(buf);
        } else {
            let cmd = match side {
                Side::Add => "DIFadd",
                Side::Del => "DIFdel",
            };
            let fl = if ctx.fl { "FL" } else { "" };
            self.push(&format!("\\{cmd}{fl}{{{buf}}}"));
        }
        buf.clear();
    }

    /// Deleted text that cannot be shown, commented out (as latexdiff's
    /// `%DIFDELCMD <` lines).
    fn comment_out(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let body = text.trim_end();
        for line in body.split('\n') {
            self.push("%DIFDELCMD < ");
            self.push(line);
            self.push("\n");
        }
        self.push("%DIFDELCMD < %%%");
        self.nl = true;
    }

    /// Whether deleted text uses a command only the old preamble defines.
    fn old_only(&self, text: &str) -> bool {
        let b = text.as_bytes();
        let mut i = 0;
        while let Some(k) = text[i..].find('\\') {
            let (name, e) = cs_at(text, i + k);
            if self.sigs.old_only(name) {
                return true;
            }
            i = e.max(i + k + 1);
            if i >= b.len() {
                break;
            }
        }
        false
    }
}

/// The paragraphs of a token sequence: runs up to and including a blank
/// line.
pub fn chunks(toks: &[Tok]) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut from = 0;
    for (i, t) in toks.iter().enumerate() {
        if t.kind == Kind::Par {
            out.push(from..i + 1);
            from = i + 1;
        }
    }
    if from < toks.len() {
        out.push(from..toks.len());
    }
    out
}

pub fn chunk_key(toks: &[Tok]) -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    for t in toks {
        t.key.hash(&mut h);
    }
    h.finish()
}

/// Drop common runs shorter than [`MIN_BLOCK`] words between two changes
/// that are much longer than they are: they become part of the change (a
/// rewrite is shown as one, not word by word).
fn merge_short(pairs: &[(usize, usize)], old: &[Tok], new: &[Tok]) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::with_capacity(pairs.len());
    let mut k = 0;
    while k < pairs.len() {
        // (a run of consecutive pairs)
        let mut e = k + 1;
        while e < pairs.len()
            && pairs[e].0 == pairs[e - 1].0 + 1
            && pairs[e].1 == pairs[e - 1].1 + 1
        {
            e += 1;
        }
        let run = &pairs[k..e];
        let (fi, fj) = run[0];
        let (li, lj) = run[run.len() - 1];
        // (the change before: since the last run kept, merged ones in it)
        let (pi, pj) = out.last().map_or((0, 0), |&(i, j)| (i + 1, j + 1));
        let before = (fi - pi) + (fj - pj);
        let (ni, nj) = pairs.get(e).copied().unwrap_or((old.len(), new.len()));
        let after = (ni - li - 1) + (nj - lj - 1);
        let words = run
            .iter()
            .all(|&(i, _)| matches!(old[i].kind, Kind::Word | Kind::Ws));
        if !(words
            && run.len() < MIN_BLOCK
            && before > 0
            && after > 0
            && before + after > 4 * run.len())
        {
            out.extend_from_slice(run);
        }
        k = e;
    }
    out
}

fn line_end(b: &[u8], i: usize) -> usize {
    b[i..]
        .iter()
        .position(|&c| c == b'\n')
        .map_or(b.len(), |k| i + k + 1)
}

/// Past the `}` closing the `{` at `i` (`%` comments and `\{` skipped).
fn brace_end(b: &[u8], i: usize) -> usize {
    let mut depth = 0usize;
    let mut k = i;
    while k < b.len() {
        match b[k] {
            b'\\' => k += 1,
            b'%' => {
                k = line_end(b, k);
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return k + 1;
                }
            }
            _ => {}
        }
        k += 1;
    }
    b.len()
}

/// The control sequence at `i` (a backslash): its name and where it ends.
fn cs_at(s: &str, i: usize) -> (&str, usize) {
    let b = s.as_bytes();
    let mut e = i + 1;
    if e < b.len() && b[e].is_ascii_alphabetic() {
        while e < b.len() && b[e].is_ascii_alphabetic() {
            e += 1;
        }
    } else if e < b.len() {
        e += s[e..].chars().next().map_or(1, char::len_utf8);
    }
    (&s[(i + 1).min(e)..e], e)
}

/// The label of an `\item[…]` (its range in `text`), if it is plain text.
fn item_label(text: &str) -> Option<(usize, usize)> {
    let rest = text.strip_prefix("\\item")?;
    let open = 5 + rest.find('[')?;
    if !text[5..open].trim().is_empty() {
        return None;
    }
    let close = open + text[open..].find(']')?;
    let label = &text[open + 1..close];
    (!label.contains(['\\', '{', '}', '%', '&', '$'])).then_some((open + 1, close))
}

/// Whether `text` is an `\item` (with its label).
fn is_item(text: &str) -> bool {
    text.strip_prefix("\\item")
        .or_else(|| text.strip_prefix("\\bibitem"))
        .is_some_and(|r| !r.starts_with(|c: char| c.is_ascii_alphabetic()))
}
