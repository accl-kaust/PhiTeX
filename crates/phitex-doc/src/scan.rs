//! Reading a paragraph's facts from its syntax tree.
//!
//! A reader walks the paragraph's tokens. A command the layer knows
//! (`known.rs`) reads its arguments as LaTeX would (a star, optional
//! arguments, then braced ones) and states a fact. Its mandatory arguments
//! must be braced, as documents write them: a command that is itself an
//! argument (`\titleformat{name=\chapter,numberless}`) is not run by TeX,
//! and is not read as a heading here. A call of
//! a macro the document defines is expanded when its expansion makes
//! facts (it is *structural*): its arguments are read, put into its body,
//! and the body is read in turn, its facts placed at the call. Any other
//! command is passed over, its arguments read as text, and its name kept
//! among the paragraph's calls: the names its facts depend on.

use std::collections::BTreeSet;
use std::rc::Rc;

use phitex_syntax::{Green, ParaId, SyntaxKind, lex};

use crate::defs::{Args, Def, Defs, Meaning, Spec, call_key, substitute, xparse};
use crate::known::{self, Construct, CounterOp, Definer, InputKind};
use crate::text;
use crate::{Fact, Name, ParaFacts, State, What};

/// How deep the layer expands the document's macros, and how many bytes
/// of expansion one paragraph may make (a macro that recurses stops there).
const MAX_DEPTH: u8 = 16;
const BUDGET: usize = 1 << 20;

/// A reader of a sequence of tokens (a paragraph's, a group's, an
/// expansion's): the token at hand, and how much of it (a text token's
/// characters) is read.
#[derive(Clone)]
pub(crate) struct Cur<'a> {
    pub text: &'a str,
    items: &'a [Green],
    idx: usize,
    /// Where `items[idx]` begins in `text`.
    off: usize,
    /// How much of `items[idx]` is read.
    skip: usize,
}

/// An argument: its text (inside its braces), and its tokens.
pub(crate) struct Arg<'a> {
    pub text: &'a str,
    items: &'a [Green],
    base: usize,
    src: &'a str,
}

impl<'a> Cur<'a> {
    pub fn new(text: &'a str, items: &'a [Green], off: usize) -> Self {
        Cur {
            text,
            items,
            idx: 0,
            off,
            skip: 0,
        }
    }

    pub fn peek(&self) -> Option<&'a Green> {
        self.items.get(self.idx)
    }

    pub fn kind(&self) -> Option<SyntaxKind> {
        self.peek().map(Green::kind)
    }

    /// Where the reader is in its text.
    pub fn pos(&self) -> usize {
        self.off + self.skip
    }

    /// Past the token at hand.
    pub fn next(&mut self) {
        if let Some(g) = self.peek() {
            self.off += g.len();
            self.idx += 1;
            self.skip = 0;
        }
    }

    /// The rest of the token at hand.
    pub fn rest(&self) -> &'a str {
        self.peek()
            .map_or("", |g| &self.text[self.off + self.skip..self.off + g.len()])
    }

    /// Past `n` bytes of the token at hand.
    fn take(&mut self, n: usize) {
        let len = self.peek().map_or(0, Green::len);
        self.skip += n;
        if self.skip >= len {
            self.next();
        }
    }

    /// Past what TeX skips before an argument: spaces, the line's end,
    /// comments.
    pub fn skip_blanks(&mut self) {
        while self.skip == 0
            && matches!(
                self.kind(),
                Some(SyntaxKind::Space | SyntaxKind::Newline | SyntaxKind::Comment)
            )
        {
            self.next();
        }
    }

    /// The next character, if a text token has it.
    fn next_char(&self) -> Option<char> {
        match self.kind()? {
            SyntaxKind::Text | SyntaxKind::Other => self.rest().chars().next(),
            _ => None,
        }
    }

    /// The control sequence at hand (a [`SyntaxKind::Cs`] token): its name,
    /// read past. A name goes on through `@` and letters after it, as
    /// LaTeX's own code reads it (`\if@twoside`, `\@sect`, `\c@page`): the
    /// tree reads with `@` not a letter.
    pub fn cs_name(&mut self) -> &'a str {
        let start = self.off + 1;
        let mut end = self.off + self.peek().map_or(1, Green::len);
        let name = &self.text[start..end];
        self.next();
        let word = name.starts_with(|c: char| c.is_ascii_alphabetic());
        if (word || name == "@") && self.kind() == Some(SyntaxKind::Text) {
            let rest = self.rest();
            if name == "@" || rest.starts_with('@') {
                let n = rest
                    .bytes()
                    .take_while(|b| b.is_ascii_alphabetic() || *b == b'@')
                    .count();
                if n > 0 {
                    self.take(n);
                    end += n;
                }
            }
        }
        &self.text[start..end]
    }

    /// `*` next (after blanks, as `\@ifstar` reads it): read past it.
    pub fn star(&mut self) -> bool {
        let save = self.clone();
        self.skip_blanks();
        if self.next_char() == Some('*') {
            self.take(1);
            true
        } else {
            *self = save;
            false
        }
    }

    /// An optional argument next: its text, read past it.
    pub fn opt(&mut self) -> Option<&'a str> {
        self.delimited('[', ']')
    }

    /// biblatex's `(note)` next.
    fn paren(&mut self) -> Option<&'a str> {
        self.delimited('(', ')')
    }

    fn delimited(&mut self, open: char, close: char) -> Option<&'a str> {
        let save = self.clone();
        self.skip_blanks();
        if self.next_char() != Some(open) {
            *self = save;
            return None;
        }
        self.take(1);
        let start = self.pos();
        loop {
            match self.kind() {
                None => {
                    *self = save;
                    return None;
                }
                Some(SyntaxKind::Text | SyntaxKind::Other) => {
                    if let Some(k) = self.rest().find(close) {
                        let end = self.pos() + k;
                        self.take(k + 1);
                        return Some(&self.text[start..end]);
                    }
                    self.next();
                }
                Some(_) => self.next(),
            }
        }
    }

    /// A mandatory argument next: a group's inside, or one token (one
    /// character of a text).
    pub fn arg(&mut self) -> Option<Arg<'a>> {
        let save = self.clone();
        self.skip_blanks();
        let Some(g) = self.peek() else {
            *self = save;
            return None;
        };
        let arg = match g.kind() {
            SyntaxKind::Group => group_arg(self.text, g, self.off),
            SyntaxKind::Text | SyntaxKind::Other => {
                let c = self.rest().chars().next()?;
                let start = self.pos();
                self.take(c.len_utf8());
                return Some(Arg {
                    text: &self.text[start..start + c.len_utf8()],
                    items: &[],
                    base: start,
                    src: self.text,
                });
            }
            SyntaxKind::Cs | SyntaxKind::Param | SyntaxKind::Verbatim => Arg {
                text: &self.text[self.off..self.off + g.len()],
                items: &self.items[self.idx..=self.idx],
                base: self.off,
                src: self.text,
            },
            _ => {
                *self = save;
                return None;
            }
        };
        self.next();
        Some(arg)
    }

    /// A braced argument next (not one token).
    fn group(&mut self) -> Option<Arg<'a>> {
        let save = self.clone();
        self.skip_blanks();
        match self.peek() {
            Some(g) if g.kind() == SyntaxKind::Group => {
                let a = group_arg(self.text, g, self.off);
                self.next();
                Some(a)
            }
            _ => {
                *self = save;
                None
            }
        }
    }

    /// A control sequence as a definition names it: `\name` or `{\name}`.
    fn cs_arg(&mut self) -> Option<&'a str> {
        self.skip_blanks();
        match self.kind()? {
            SyntaxKind::Cs => Some(self.cs_name()),
            SyntaxKind::Group => {
                let a = self.group()?;
                let mut c = Cur::new(a.src, a.items, a.base);
                c.skip_blanks();
                (c.kind() == Some(SyntaxKind::Cs)).then(|| c.cs_name())
            }
            _ => None,
        }
    }

    /// Past the spaces after a control word (TeX's input skips them).
    fn skip_spaces(&mut self) {
        while self.skip == 0 && self.kind() == Some(SyntaxKind::Space) {
            self.next();
        }
    }
}

/// A group's inside as an argument.
fn group_arg<'a>(text: &'a str, g: &'a Green, off: usize) -> Arg<'a> {
    let ch = g.children();
    let closed = ch.len() >= 2 && ch.last().map(Green::kind) == Some(SyntaxKind::Brace);
    let inner = &ch[1.min(ch.len())..ch.len() - usize::from(closed)];
    let start = off + 1;
    let end = off + g.len() - usize::from(closed);
    Arg {
        text: &text[start..end.max(start)],
        items: inner,
        base: start,
        src: text,
    }
}

/// The groups of an argument (`\graphicspath{{a/}{b/}}`): their insides.
fn groups_in(a: &Arg<'_>) -> Vec<String> {
    let mut c = Cur::new(a.src, a.items, a.base);
    let mut out = Vec::new();
    while let Some(g) = c.group() {
        out.push(g.text.trim().to_owned());
    }
    out
}

/// The reader of a paragraph's facts.
pub(crate) struct Scan<'d> {
    defs: &'d Defs,
    facts: Vec<Fact>,
    calls: BTreeSet<Name>,
    state: State,
    /// Inside an expansion: the offset of the call it came from.
    at: Option<u32>,
    via: Option<Rc<[Name]>>,
    depth: u8,
    budget: usize,
    /// `\endinput` read: reading stops at the line's end.
    end_line: bool,
}

/// A paragraph's facts, read from `entry`.
pub(crate) fn para_facts(
    defs: &Defs,
    id: ParaId,
    text: &str,
    green: &Green,
    entry: State,
    epoch: u64,
) -> ParaFacts {
    let mut s = Scan::new(defs, entry);
    if !entry.ended {
        s.walk(&mut Cur::new(text, green.children(), 0));
        if s.end_line {
            s.state.ended = true;
        }
    }
    ParaFacts {
        id,
        entry,
        exit: s.state,
        facts: s.facts,
        calls: s.calls.into_iter().collect(),
        newlines: text::newlines(text),
        epoch,
    }
}

/// The names a body calls (as [`call_key`]s).
pub(crate) fn body_calls(src: &str) -> Vec<Name> {
    fn walk(c: &mut Cur<'_>, out: &mut BTreeSet<Name>) {
        while let Some(g) = c.peek() {
            match g.kind() {
                SyntaxKind::Cs => {
                    let name = c.cs_name();
                    if name == "begin" || name == "end" {
                        if let Some(a) = c.arg() {
                            out.insert(call_key(true, a.text.trim()));
                        }
                    } else if !out.contains(name) {
                        out.insert(Name::from(name));
                    }
                }
                SyntaxKind::Group => {
                    let mut inner = Cur::new(c.text, g.children(), c.off);
                    c.next();
                    walk(&mut inner, out);
                }
                _ => c.next(),
            }
        }
    }
    let green = lex(src);
    let mut out = BTreeSet::new();
    walk(&mut Cur::new(src, green.children(), 0), &mut out);
    out.into_iter().collect()
}

/// Whether calling the document's macro (or environment) `name` makes
/// facts, so that the layer expands it: its body makes one, or calls a
/// macro that does (a cycle does not).
pub(crate) fn structural(defs: &Defs, env: bool, name: &str) -> bool {
    let key = call_key(env, name);
    if let Some(&b) = defs.structural.borrow().get(&key) {
        return b;
    }
    // (not structural while being decided: a recursion stops here)
    defs.structural.borrow_mut().insert(key.clone(), false);
    let b = match defs.meaning(env, name) {
        Meaning::One(def) => {
            let def = def.clone();
            decide(defs, &def)
        }
        _ => false,
    };
    defs.structural.borrow_mut().insert(key, b);
    b
}

fn decide(defs: &Defs, def: &Def) -> bool {
    if let Some(t) = &def.target {
        return known::construct(t).is_some() || structural(defs, false, t);
    }
    if def.args == Args::Unread {
        return false;
    }
    let probe = |src: &str| {
        let green = lex(src);
        let mut s = Scan::new(defs, State::default());
        // (read, not expanded: what it calls is decided in turn)
        s.depth = MAX_DEPTH;
        s.walk(&mut Cur::new(src, green.children(), 0));
        makes_structure(&s.facts, def.env)
            || s.calls.iter().any(|c| match c.strip_prefix('{') {
                Some(e) => structural(defs, true, e.trim_end_matches('}')),
                None => structural(defs, false, c),
            })
    };
    probe(&def.body) || (def.env && probe(&def.end))
}

/// Whether facts make structure: anything but definitions and (in an
/// environment's code, or balanced in a macro's) environments.
fn makes_structure(facts: &[Fact], env: bool) -> bool {
    let mut balance = 0i32;
    for f in facts {
        match &f.what {
            What::Define(_) | What::DynamicDefine => {}
            What::Begin(_) => balance += 1,
            What::End(_) => balance -= 1,
            What::If { then, other, .. } => {
                if makes_structure(then, env) || makes_structure(other, env) {
                    return true;
                }
            }
            _ => return true,
        }
    }
    !env && balance != 0
}

/// A macro's arguments, read from the call (`None`: one is missing, or
/// the layer does not read its parameters).
fn read_args(args: &Args, cur: &mut Cur<'_>) -> Option<Vec<String>> {
    let mut out = Vec::new();
    match args {
        Args::Latex { n, default } => {
            let mut n = *n;
            if let Some(d) = default
                && n > 0
            {
                out.push(cur.opt().map_or_else(|| d.clone(), str::to_owned));
                n -= 1;
            }
            for _ in 0..n {
                out.push(cur.arg()?.text.to_owned());
            }
        }
        Args::Plain(n) => {
            for _ in 0..*n {
                out.push(cur.arg()?.text.to_owned());
            }
        }
        Args::Xparse(specs) => {
            for s in specs {
                out.push(match s {
                    Spec::M => cur.arg()?.text.to_owned(),
                    Spec::O(d) => cur
                        .opt()
                        .map(str::to_owned)
                        .or_else(|| d.clone())
                        .unwrap_or_else(|| "-NoValue-".to_owned()),
                    Spec::S => if cur.star() {
                        "\\BooleanTrue"
                    } else {
                        "\\BooleanFalse"
                    }
                    .to_owned(),
                });
            }
        }
        Args::Unread => return None,
    }
    Some(out)
}

/// The control sequence a body without parameters is, if it is only one
/// (`\newcommand{\sect}{\section}`).
fn alias(body: &str, args: &Args) -> Option<Name> {
    if !matches!(args, Args::Latex { n: 0, .. } | Args::Plain(0)) {
        return None;
    }
    let b = body.trim();
    let name = b.strip_prefix('\\')?;
    (!name.is_empty() && name.bytes().all(|c| c.is_ascii_alphabetic() || c == b'@'))
        .then(|| Name::from(name))
}

/// The sectioning counters, and `secnumdepth`: those the outline's
/// numbers read.
fn sectioning(counter: &str) -> bool {
    counter == "secnumdepth" || known::LEVELS.contains(&counter)
}

impl<'d> Scan<'d> {
    fn new(defs: &'d Defs, state: State) -> Self {
        Scan {
            defs,
            facts: Vec::new(),
            calls: BTreeSet::new(),
            state,
            at: None,
            via: None,
            depth: 0,
            budget: BUDGET,
            end_line: false,
        }
    }

    fn walk(&mut self, cur: &mut Cur<'_>) {
        while let Some(g) = cur.peek() {
            if self.state.ended {
                return;
            }
            match g.kind() {
                SyntaxKind::Cs => {
                    let at = cur.pos();
                    let name = cur.cs_name();
                    self.command(name, at, cur);
                }
                SyntaxKind::Group => {
                    let mut inner = Cur::new(cur.text, g.children(), cur.off);
                    cur.next();
                    self.walk(&mut inner);
                }
                SyntaxKind::Newline => {
                    cur.next();
                    if self.end_line && self.at.is_none() {
                        self.state.ended = true;
                        return;
                    }
                }
                _ => cur.next(),
            }
        }
    }

    fn walk_arg(&mut self, a: &Arg<'_>) {
        self.walk(&mut Cur::new(a.src, a.items, a.base));
    }

    fn push(&mut self, at: u32, cs: &'static str, what: What) {
        self.facts.push(Fact {
            at,
            cs,
            via: self.via.clone(),
            what,
        });
    }

    /// The facts `f` states, apart.
    fn collect(&mut self, f: impl FnOnce(&mut Self)) -> Vec<Fact> {
        let saved = std::mem::take(&mut self.facts);
        f(self);
        std::mem::replace(&mut self.facts, saved)
    }

    fn call(&mut self, key: &str) {
        if !self.calls.contains(key) {
            self.calls.insert(Name::from(key));
        }
    }

    /// `name` added to the macros the facts come through; the old list.
    fn push_via(&mut self, name: &str) -> Option<Rc<[Name]>> {
        let mut v: Vec<Name> = self.via.as_deref().map_or_else(Vec::new, <[Name]>::to_vec);
        v.push(Name::from(name));
        self.via.replace(v.into())
    }

    fn command(&mut self, name: &str, at: usize, cur: &mut Cur<'_>) {
        if self.state.skip > 0 {
            if known::is_conditional(name) {
                self.state.skip += 1;
            } else if name == "fi" {
                self.state.skip -= 1;
            } else if name == "else" && self.state.skip == 1 {
                self.state.skip = 0;
            }
            return;
        }
        let at = self
            .at
            .unwrap_or_else(|| u32::try_from(at).unwrap_or(u32::MAX));
        if let Some((cs, c)) = known::construct(name) {
            self.construct(c, cs, at, cur);
            return;
        }
        self.call(name);
        let def = match self.defs.meaning(false, name) {
            Meaning::One(def) => def.clone(),
            _ => return,
        };
        if self.depth >= MAX_DEPTH {
            return;
        }
        if let Some(target) = &def.target {
            // (an alias: of LaTeX's command, or of another macro)
            let old = self.push_via(name);
            self.depth += 1;
            if let Some((cs, c)) = known::construct(target) {
                self.construct(c, cs, at, cur);
            } else {
                self.command(target, at as usize, cur);
            }
            self.depth -= 1;
            self.via = old;
            return;
        }
        if structural(self.defs, false, name) {
            let save = cur.clone();
            match read_args(&def.args, cur) {
                Some(args) => self.expand(&substitute(&def.body, &args), name, at),
                None => *cur = save,
            }
        }
    }

    /// Read `body`, the expansion of a call of `name` at `at`: its facts
    /// are placed at the call. A conditional it opens ends with it.
    fn expand(&mut self, body: &str, name: &str, at: u32) {
        if body.len() > self.budget {
            return;
        }
        self.budget -= body.len();
        let green = lex(body);
        let old_via = self.push_via(name);
        let old_at = self.at.replace(at);
        let old_state = self.state;
        self.state.skip = 0;
        self.depth += 1;
        self.walk(&mut Cur::new(body, green.children(), 0));
        self.depth -= 1;
        self.state = old_state;
        self.at = old_at;
        self.via = old_via;
    }

    #[allow(clippy::too_many_lines)]
    fn construct(&mut self, c: Construct, cs: &'static str, at: u32, cur: &mut Cur<'_>) {
        let w = |a: &Arg<'_>| text::written(a.text).trim().to_owned();
        match c {
            Construct::Heading(level) => {
                let star = cur.star();
                let short = cur.opt().map(str::to_owned);
                if short.is_some() {
                    // (memoir's `\chapter[toc][head]{title}`)
                    let _ = cur.opt();
                }
                let Some(title) = cur.group() else { return };
                let end = self
                    .at
                    .unwrap_or_else(|| u32::try_from(cur.pos()).unwrap_or(u32::MAX));
                let title_at = if self.at.is_none() {
                    u32::try_from(title.base).ok()
                } else {
                    None
                };
                self.push(
                    at,
                    cs,
                    What::Heading {
                        level,
                        star,
                        short,
                        title: title.text.to_owned(),
                        end,
                        title_at,
                    },
                );
                self.walk_arg(&title);
            }
            Construct::AddContentsLine => {
                let Some(list) = cur.group() else { return };
                let Some(level) = cur.group() else { return };
                let Some(title) = cur.group() else { return };
                self.push(
                    at,
                    cs,
                    What::Toc {
                        list: w(&list),
                        level: w(&level),
                        title: title.text.to_owned(),
                    },
                );
            }
            Construct::Label => {
                let _ = cur.opt();
                if let Some(k) = cur.group() {
                    self.push(at, cs, What::Label(text::key(k.text)));
                }
            }
            Construct::Ref { list, two, opt_key } => {
                cur.star();
                if opt_key {
                    if let Some(k) = cur.opt() {
                        self.push(at, cs, What::Ref(vec![text::key(k)]));
                    }
                    return;
                }
                let _ = cur.opt();
                let _ = cur.opt();
                let Some(k) = cur.group() else { return };
                let mut keys = if list {
                    text::list(k.text)
                } else {
                    vec![text::key(k.text)]
                };
                if two && let Some(k2) = cur.group() {
                    keys.push(text::key(k2.text));
                }
                self.push(at, cs, What::Ref(keys));
            }
            Construct::Cite { multi, nocite } => {
                cur.star();
                let mut keys = Vec::new();
                if multi {
                    let _ = cur.paren();
                    let _ = cur.paren();
                    loop {
                        let _ = cur.opt();
                        let _ = cur.opt();
                        match cur.group() {
                            Some(k) => keys.extend(text::list(k.text)),
                            None => break,
                        }
                    }
                } else {
                    let _ = cur.opt();
                    let _ = cur.opt();
                    if let Some(k) = cur.group() {
                        keys = text::list(k.text);
                    }
                }
                self.push(at, cs, What::Cite { keys, nocite });
            }
            Construct::Input(how) => self.input(how, cs, at, cur),
            Construct::InputIfFileExists | Construct::IfFileExists => {
                let Some(f) = cur.group() else { return };
                let path = w(&f);
                let then = cur.group();
                let other = cur.group();
                let mut then = self.collect(|s| {
                    if let Some(t) = &then {
                        s.walk_arg(t);
                    }
                });
                if c == Construct::InputIfFileExists {
                    then.push(Fact {
                        at,
                        cs,
                        via: self.via.clone(),
                        what: What::Input {
                            path: path.clone(),
                            how: InputKind::Input,
                            sub: false,
                        },
                    });
                }
                let other = self.collect(|s| {
                    if let Some(o) = &other {
                        s.walk_arg(o);
                    }
                });
                self.push(
                    at,
                    cs,
                    What::If {
                        test: path,
                        then,
                        other,
                    },
                );
            }
            Construct::IncludeOnly => {
                if let Some(a) = cur.group() {
                    self.push(at, cs, What::IncludeOnly(text::list(a.text)));
                }
            }
            Construct::Bibliography => {
                // (BibTeX's `.bib` added, unless there: compared as BibTeX
                // compares, with case)
                #[allow(clippy::case_sensitive_file_extension_comparisons)]
                if let Some(a) = cur.group() {
                    let files = text::list(a.text)
                        .into_iter()
                        .map(|f| {
                            if f.ends_with(".bib") {
                                f
                            } else {
                                format!("{f}.bib")
                            }
                        })
                        .collect();
                    self.push(at, cs, What::Bib(files));
                }
            }
            Construct::AddBibResource => {
                let _ = cur.opt();
                if let Some(a) = cur.group() {
                    self.push(at, cs, What::Bib(vec![w(&a)]));
                }
            }
            Construct::Asset(how) => {
                cur.star();
                let opts = cur.opt();
                let _ = cur.opt();
                if cs == "inputminted" {
                    // (the language)
                    let _ = cur.group();
                }
                let Some(f) = cur.group() else { return };
                self.push(at, cs, What::Asset { path: w(&f), how });
                if cs == "lstinputlisting"
                    && let Some(l) = opts.and_then(|o| text::keyval(o, "label"))
                {
                    self.push(at, cs, What::Label(l));
                }
            }
            Construct::GraphicsPath => {
                if let Some(a) = cur.group() {
                    self.push(at, cs, What::GraphicsPath(groups_in(&a)));
                }
            }
            Construct::Begin => self.begin(cs, at, cur),
            Construct::End => self.end(cs, at, cur),
            Construct::Define(d) => self.define(d, cs, at, cur),
            Construct::Counter(op) => {
                let Some(n) = cur.group() else { return };
                let name = w(&n);
                let value = if op == CounterOp::Step {
                    None
                } else {
                    let Some(v) = cur.group() else { return };
                    text::integer(v.text)
                };
                if sectioning(&name) {
                    self.push(at, cs, What::Counter { name, op, value });
                }
            }
            Construct::Matter(m) => self.push(at, cs, What::Matter(m)),
            Construct::DocumentClass => {
                let _ = cur.opt();
                if let Some(a) = cur.group() {
                    self.push(at, cs, What::Class(w(&a)));
                }
            }
            Construct::UsePackage => {
                let _ = cur.opt();
                if let Some(a) = cur.group() {
                    self.push(at, cs, What::Packages(text::list(a.text)));
                }
            }
            Construct::IfFalse => self.state.skip = 1,
            Construct::EndInput => {
                if self.at.is_none() {
                    self.end_line = true;
                }
            }
        }
    }

    fn input(&mut self, how: InputKind, cs: &'static str, at: u32, cur: &mut Cur<'_>) {
        let w = |a: &Arg<'_>| text::written(a.text).trim().to_owned();
        let mut sub = false;
        let path = match how {
            InputKind::Input => {
                cur.skip_blanks();
                match cur.kind() {
                    Some(SyntaxKind::Group) => cur.group().map(|a| w(&a)),
                    // (TeX's `\input file`: the name up to a space)
                    Some(SyntaxKind::Text) => {
                        let name = cur.rest().to_owned();
                        cur.next();
                        Some(name)
                    }
                    _ => None,
                }
            }
            InputKind::Import => {
                sub = cs.starts_with("sub");
                let Some(d) = cur.group() else { return };
                let Some(f) = cur.group() else { return };
                Some(text::join(&w(&d), &w(&f)))
            }
            _ => cur.group().map(|a| w(&a)),
        };
        if let Some(path) = path.filter(|p| !p.is_empty()) {
            self.push(at, cs, What::Input { path, how, sub });
        }
    }

    fn begin(&mut self, cs: &'static str, at: u32, cur: &mut Cur<'_>) {
        let Some(n) = cur.group() else { return };
        let name = text::written(n.text).trim().to_owned();
        self.push(at, cs, What::Begin(name.clone()));
        match name.as_str() {
            "filecontents" | "filecontents*" => {
                let _ = cur.opt();
                if let Some(f) = cur.group() {
                    let path = text::written(f.text).trim().to_owned();
                    if cur.kind() == Some(SyntaxKind::Verbatim) {
                        let body = cur.rest().to_owned();
                        cur.next();
                        self.push(at, cs, What::Embedded { path, text: body });
                    }
                }
            }
            "lstlisting" => {
                if let Some(l) = cur.opt().and_then(|o| text::keyval(o, "label")) {
                    self.push(at, cs, What::Label(l));
                }
            }
            _ => self.environment(&name, true, at, cur),
        }
    }

    fn end(&mut self, cs: &'static str, at: u32, cur: &mut Cur<'_>) {
        let Some(n) = cur.group() else { return };
        let name = text::written(n.text).trim().to_owned();
        self.environment(&name, false, at, cur);
        self.push(at, cs, What::End(name.clone()));
        if name == "document" && self.at.is_none() {
            self.state.ended = true;
        }
    }

    /// The document's own environment `name` begun (or ended): its code
    /// read, when it makes facts.
    fn environment(&mut self, name: &str, begin: bool, at: u32, cur: &mut Cur<'_>) {
        let key = call_key(true, name);
        self.call(&key);
        let def = match self.defs.meaning(true, name) {
            Meaning::One(def) => def.clone(),
            _ => return,
        };
        if self.depth >= MAX_DEPTH || !structural(self.defs, true, name) {
            return;
        }
        if begin {
            let save = cur.clone();
            match read_args(&def.args, cur) {
                Some(args) => self.expand(&substitute(&def.body, &args), &key, at),
                None => *cur = save,
            }
        } else {
            self.expand(&def.end, &key, at);
        }
    }

    #[allow(clippy::too_many_lines)]
    fn define(&mut self, d: Definer, cs: &'static str, at: u32, cur: &mut Cur<'_>) {
        let latex_args = |cur: &mut Cur<'_>| {
            let n = cur
                .opt()
                .and_then(|t| t.trim().parse::<u8>().ok())
                .unwrap_or(0);
            let default = if n > 0 {
                cur.opt().map(str::to_owned)
            } else {
                None
            };
            Args::Latex { n, default }
        };
        let def = match d {
            Definer::NewCommand { .. } => {
                cur.star();
                let Some(name) = cur.cs_arg() else { return };
                let args = latex_args(cur);
                let Some(body) = cur.arg() else { return };
                new_def(name, false, cs, d, args, body.text, "")
            }
            Definer::DocumentCommand => {
                let Some(name) = cur.cs_arg() else { return };
                let Some(spec) = cur.arg() else { return };
                let Some(body) = cur.arg() else { return };
                new_def(name, false, cs, d, xparse(spec.text), body.text, "")
            }
            Definer::Def => {
                cur.skip_blanks();
                if cur.kind() != Some(SyntaxKind::Cs) {
                    return;
                }
                let name = cur.cs_name();
                if name == "csname" {
                    // (`\expandafter\def\csname…\endcsname`: a name the
                    // layer cannot know)
                    while let Some(g) = cur.peek() {
                        if g.kind() == SyntaxKind::Cs && cur.cs_name() == "endcsname" {
                            break;
                        }
                        if g.kind() != SyntaxKind::Cs {
                            cur.next();
                        }
                    }
                    while cur.peek().is_some_and(|g| g.kind() != SyntaxKind::Group) {
                        cur.next();
                    }
                    let _ = cur.arg();
                    self.push(at, cs, What::DynamicDefine);
                    return;
                }
                cur.skip_spaces();
                // (the parameter text, up to the body: `#1#2` only, or
                // delimiters the layer does not read)
                let mut n = 0u8;
                let mut plain = true;
                while let Some(g) = cur.peek() {
                    match g.kind() {
                        SyntaxKind::Group => break,
                        SyntaxKind::Param if plain && cur.rest() == format!("#{}", n + 1) => {
                            n += 1;
                        }
                        _ => plain = false,
                    }
                    cur.next();
                }
                let Some(body) = cur.arg() else { return };
                let args = if plain { Args::Plain(n) } else { Args::Unread };
                new_def(name, false, cs, d, args, body.text, "")
            }
            Definer::Let => {
                cur.skip_blanks();
                if cur.kind() != Some(SyntaxKind::Cs) {
                    return;
                }
                let name = cur.cs_name();
                cur.skip_blanks();
                if cur.next_char() == Some('=') {
                    cur.take(1);
                    cur.skip_blanks();
                }
                let target = if cur.kind() == Some(SyntaxKind::Cs) {
                    Some(Name::from(cur.cs_name()))
                } else {
                    let _ = cur.arg();
                    None
                };
                let mut def = new_def(name, false, cs, d, Args::Plain(0), "", "");
                def.target = target;
                def
            }
            Definer::NewEnvironment | Definer::DocumentEnvironment => {
                cur.star();
                let Some(name) = cur.arg() else { return };
                let name = text::written(name.text).trim().to_owned();
                let args = if d == Definer::NewEnvironment {
                    latex_args(cur)
                } else {
                    let Some(spec) = cur.arg() else { return };
                    xparse(spec.text)
                };
                let Some(b) = cur.arg() else { return };
                let Some(e) = cur.arg() else { return };
                new_def(&name, true, cs, d, args, b.text, e.text)
            }
            Definer::OtherEnvironment | Definer::VerbatimEnvironment => {
                let Some(name) = Self::other_environment(cs, cur) else {
                    return;
                };
                new_def(&name, true, cs, d, Args::Unread, "", "")
            }
        };
        self.push(at, cs, What::Define(Rc::new(def)));
    }

    /// The environment a package's command defines, its arguments read past.
    fn other_environment(cs: &str, cur: &mut Cur<'_>) -> Option<String> {
        let name = |a: Option<Arg<'_>>| a.map(|a| text::written(a.text).trim().to_owned());
        let n = match cs {
            "newtheorem" => {
                let n = name(cur.arg());
                let _ = cur.opt();
                let _ = cur.arg();
                let _ = cur.opt();
                n
            }
            "NewEnviron" | "RenewEnviron" => {
                let n = name(cur.arg());
                let _ = cur.opt();
                let _ = cur.opt();
                let _ = cur.arg();
                let _ = cur.opt();
                n
            }
            "newmdenv" => {
                let _ = cur.opt();
                name(cur.arg())
            }
            "lstnewenvironment" => {
                let n = name(cur.arg());
                let _ = cur.opt();
                let _ = cur.opt();
                let _ = cur.arg();
                let _ = cur.arg();
                n
            }
            "DefineVerbatimEnvironment" => {
                let n = name(cur.arg());
                let _ = cur.arg();
                let _ = cur.arg();
                n
            }
            "newminted" => {
                let env = cur.opt().map(|o| o.trim().to_owned());
                let lang = name(cur.arg());
                let _ = cur.arg();
                env.or_else(|| lang.map(|l| format!("{l}code")))
            }
            // (tcolorbox's `\newtcolorbox[init]{name}[n][default]{options}`)
            _ => {
                let _ = cur.opt();
                let n = name(cur.arg());
                let _ = cur.opt();
                let _ = cur.opt();
                let _ = cur.arg();
                n
            }
        };
        n.filter(|n| !n.is_empty())
    }
}

/// A definition, with what its code calls and the control sequence it
/// stands for if it is only one.
fn new_def(
    name: &str,
    env: bool,
    cs: &str,
    definer: Definer,
    args: Args,
    body: &str,
    end: &str,
) -> Def {
    let mut calls = body_calls(body);
    if env {
        calls.extend(body_calls(end));
        calls.sort();
        calls.dedup();
    }
    let target = if env { None } else { alias(body, &args) };
    Def {
        name: Name::from(name),
        env,
        cs: Name::from(cs),
        definer,
        args,
        body: body.to_owned(),
        end: end.to_owned(),
        target,
        calls,
    }
}

/// A text with the document's macros expanded as LaTeX's
/// `\protected@write` expands them when it writes a title to the `.toc`:
/// a `\newcommand` or `\def` macro is, a robust one (or one after
/// `\protect`) is not; LaTeX's own commands are kept as they are, but for
/// hyperref's `\texorpdfstring{tex}{pdf}`, which writes `tex`.
pub(crate) fn expand_text(defs: &Defs, src: &str, depth: u8) -> String {
    let green = lex(src);
    let mut out = String::with_capacity(src.len());
    expand_into(
        defs,
        &mut out,
        &mut Cur::new(src, green.children(), 0),
        depth,
    );
    out
}

fn expand_into(defs: &Defs, out: &mut String, cur: &mut Cur<'_>, depth: u8) {
    let mut protect = false;
    while let Some(g) = cur.peek() {
        match g.kind() {
            SyntaxKind::Cs => {
                let start = cur.pos();
                let name = cur.cs_name();
                let protected = std::mem::replace(&mut protect, name == "protect");
                if name == "texorpdfstring" {
                    // (hyperref's: its first argument where TeX writes)
                    let save = cur.clone();
                    if let (Some(tex), Some(_)) = (cur.arg(), cur.arg()) {
                        out.push_str(&expand_text(defs, tex.text, depth + 1));
                        continue;
                    }
                    *cur = save;
                }
                if !protected
                    && depth < MAX_DEPTH
                    && let Meaning::One(def) = defs.meaning(false, name)
                    && def.expands_in_writes()
                {
                    let def = def.clone();
                    let save = cur.clone();
                    if name.starts_with(|c: char| c.is_ascii_alphabetic()) {
                        // (TeX's input skips the blanks after a control word)
                        cur.skip_spaces();
                        if cur.kind() == Some(SyntaxKind::Newline) {
                            cur.next();
                            cur.skip_spaces();
                        }
                    }
                    if let Some(args) = read_args(&def.args, cur) {
                        let body = substitute(&def.body, &args);
                        out.push_str(&expand_text(defs, &body, depth + 1));
                        continue;
                    }
                    *cur = save;
                }
                out.push_str(&cur.text[start..cur.pos()]);
            }
            SyntaxKind::Group => {
                let a = group_arg(cur.text, g, cur.off);
                let closed = a.base + a.text.len() < cur.off + g.len();
                out.push('{');
                expand_into(defs, out, &mut Cur::new(a.src, a.items, a.base), depth);
                if closed {
                    out.push('}');
                }
                cur.next();
                protect = false;
            }
            _ => {
                out.push_str(cur.rest());
                cur.next();
                if !matches!(g.kind(), SyntaxKind::Space | SyntaxKind::Newline) {
                    protect = false;
                }
            }
        }
    }
}
