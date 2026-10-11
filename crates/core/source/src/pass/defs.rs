//! Definitions visible in text: what each defined name's uses consume,
//! read from its parameter text (`\def`), its argument count and default
//! (`\newcommand`), its argument spec (`\NewDocumentCommand`), or what it
//! was declared as (`\newcount`, `\newif`, `\let`).

use std::collections::{HashMap, HashSet};

use super::FileKind;
use super::atoms::{self, Atom, AtomKind};
use super::lex::{self, changes_catcodes};

/// One argument a command reads (see the table module's spec letters).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arg {
    Star,
    /// An optional character (xparse's `t`).
    OptChar(char),
    Opt,
    M,
    Tok,
    Relation,
    ParamText,
    Group,
    BoxSpec,
    Eq,
    Int,
    Dimen,
    Glue,
    Value,
    Reg,
    CsName,
    Expr,
    File,
    Toks,
    Rule,
    By,
    To,
    LetTarget,
    FontSize,
    Keywords,
    Tikz,
    Semicolon,
    Verb,
    Env,
    StarSkip,
    UntilCs(String),
    /// Parameter text before `#1`: these tokens must follow.
    Literal(Vec<Lit>),
    /// An argument delimited by these tokens.
    Delimited(Vec<Lit>),
    /// An argument delimited by `{` (`#1#{`).
    UntilOpen,
    Pending(String),
}

/// A token of parameter text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lit {
    Cs(String),
    Char(char),
}

/// What a defined name is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shape {
    Macro(Vec<Arg>),
    /// `\let` to another command.
    Alias(String),
    /// A register or a constant: `'i'`, `'d'`, `'g'`, `'t'`, `'c'`.
    Quantity(char),
    /// A conditional (`\newif`).
    Cond,
    /// Defined in a way this pass cannot read.
    Opaque(String),
}

#[derive(Clone, Debug)]
struct At<T> {
    file: u32,
    pos: usize,
    what: T,
}

/// The definitions harvested from a set of files.
#[derive(Clone, Debug, Default)]
pub struct Definitions {
    macros: HashMap<String, Vec<At<Shape>>>,
    envs: HashMap<String, Vec<At<Vec<Arg>>>>,
    /// Per definition, whether its body changes catcodes: 0 no, 1 inside
    /// a group of its own (for the arguments it reads), 2 for good, 3 only
    /// of the characters in the mask (none that shape groups).
    catcoders: HashMap<String, Vec<At<(u8, u128)>>>,
    env_catcoders: HashSet<String>,
    /// Characters some `\catcode` makes active.
    active: u128,
}

pub(crate) enum Found<'a, T> {
    None,
    One(&'a T),
    Conflict,
}

fn find<T: PartialEq>(v: Option<&Vec<At<T>>>, at: Option<(u32, usize)>) -> Found<'_, T> {
    let Some(v) = v else { return Found::None };
    let Some((file, pos)) = at else {
        return v.last().map_or(Found::None, |d| Found::One(&d.what));
    };
    // (document order binds: the nearest definition before the use in its
    // file; otherwise the definitions must agree)
    if let Some(d) = v
        .iter()
        .filter(|d| d.file == file && d.pos < pos)
        .max_by_key(|d| d.pos)
    {
        return Found::One(&d.what);
    }
    if v.iter().all(|d| d.what == v[0].what) {
        Found::One(&v[0].what)
    } else {
        Found::Conflict
    }
}

impl Definitions {
    #[must_use]
    pub fn new() -> Definitions {
        Definitions::default()
    }

    /// How many names are defined.
    #[must_use]
    pub fn len(&self) -> usize {
        self.macros.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.macros.is_empty()
    }

    /// What `name` is at `at` (file, byte) or, without a place, by its
    /// last definition (a format's).
    pub(crate) fn find(&self, name: &str, at: Option<(u32, usize)>) -> Found<'_, Shape> {
        find(self.macros.get(name), at)
    }

    /// Whether the document defines `name` anywhere.
    pub(crate) fn defines(&self, name: &str) -> bool {
        self.macros.contains_key(name)
    }

    /// Whether `name` is defined in `file` before `pos`.
    pub(crate) fn defined_before(&self, name: &str, file: u32, pos: usize) -> bool {
        self.macros
            .get(name)
            .is_some_and(|v| v.iter().any(|d| d.file == file && d.pos < pos))
    }

    pub(crate) fn find_env(&self, name: &str, at: Option<(u32, usize)>) -> Found<'_, Vec<Arg>> {
        find(self.envs.get(name), at)
    }

    /// The characters the document makes active somewhere.
    pub(crate) fn maybe_active(&self) -> u128 {
        self.active
    }

    /// Whether the definition of `name` bound at `at` changes catcodes
    /// (0 no, 1 for the arguments it reads, 2 for good).
    pub(crate) fn changes_catcodes(&self, name: &str, at: Option<(u32, usize)>) -> (u8, u128) {
        match find(self.catcoders.get(name), at) {
            Found::One(k) => *k,
            Found::None => (0, 0),
            Found::Conflict => {
                // (bound to one of several definitions: the worst)
                self.catcoders.get(name).map_or((0, 0), |v| {
                    let k = v
                        .iter()
                        .map(|d| d.what.0)
                        .filter(|&k| k != 3)
                        .max()
                        .unwrap_or(0);
                    let m = v.iter().fold(0, |m, d| m | d.what.1);
                    if k == 0 && m != 0 { (3, m) } else { (k, m) }
                })
            }
        }
    }

    pub(crate) fn env_changes_catcodes(&self, name: &str) -> bool {
        self.env_catcoders.contains(name)
    }

    fn add(&mut self, name: &str, file: u32, pos: usize, what: Shape) {
        // (`\def\noexpand\x` in an `\edef` defines `\x`, not
        // `\noexpand`; `\let\x\@undefined` undefines)
        if matches!(
            name,
            "noexpand"
                | "expandafter"
                | "csname"
                | "string"
                | "meaning"
                | "global"
                | "long"
                | "protected"
                | "exp_not:N"
                | "exp_after:wN"
                | "unexpanded"
                | "the"
        ) || matches!(&what, Shape::Alias(t) if matches!(t.as_str(), "@undefined" | "undefined" | "relax" | "@empty"))
            && self.macros.contains_key(name)
        {
            return;
        }
        let v = self.macros.entry(name.to_owned()).or_default();
        if !v.iter().any(|d| d.file == file && d.what == what) || v.iter().all(|d| d.file != file) {
            v.push(At { file, pos, what });
        }
    }

    fn catcodes(&mut self, name: &str, file: u32, pos: usize, k: (u8, u128)) {
        if !matches!(
            name,
            "noexpand" | "expandafter" | "csname" | "string" | "meaning" | "the"
        ) {
            self.catcoders
                .entry(name.to_owned())
                .or_default()
                .push(At { file, pos, what: k });
        }
    }

    /// Reads the definitions in `text`, file `file` of the document.
    pub fn harvest(&mut self, text: &str, kind: FileKind, file: u32) {
        let lexed = lex::lex(text, kind, None);
        let atoms = atoms::atoms(text, &lexed.tokens);
        for t in &lexed.tokens {
            if t.kind == lex::TokKind::Cs
                && &text[t.start..t.end] == "\\catcode"
                && let Some((c, Some(13))) = lex::catcode_assignment(text.as_bytes(), t.end)
                && c < 128
                && c != b'~'
                && !lex::structural(c)
            {
                self.active |= 1 << c;
            }
        }
        let mut a = 0;
        while a < atoms.len() {
            if atoms[a].kind == AtomKind::Cs {
                self.definition(text, &atoms, a, file);
            }
            a += 1;
        }
    }

    #[allow(clippy::too_many_lines)]
    fn definition(&mut self, text: &str, atoms: &[Atom], a: usize, file: u32) {
        let name = atoms[a].name(text);
        let pos = atoms[a].start;
        let body_catcodes = |open: usize| -> (u8, u128) {
            let close = atoms[open].mate;
            if close == usize::MAX {
                return (0, 0);
            }
            let body = &atoms[open + 1..close];
            let is =
                |x: &Atom, names: &[&str]| x.kind == AtomKind::Cs && names.contains(&x.name(text));
            // (the characters changed, when each is literal and none shapes
            // groups; `\makeatletter` changes only whether `@` is a letter
            // and is not counted)
            let mut first = None;
            let mut chars = 0u128;
            let mut shaping = false;
            for (k, x) in body.iter().enumerate() {
                if x.kind != AtomKind::Cs {
                    continue;
                }
                let n = x.name(text);
                if !changes_catcodes(n)
                    || matches!(
                        n,
                        "makeatletter" | "makeatother" | "ExplSyntaxOn" | "ExplSyntaxOff"
                    )
                {
                    continue;
                }
                first.get_or_insert(k);
                let mut j = k + 1;
                if n == "catcode" || n == "tex_catcode:D" {
                    if body.get(j).map(|y| y.kind) != Some(AtomKind::Char('`')) {
                        shaping = true;
                        continue;
                    }
                    j += 1;
                } else if !(n == "@makeother" || n.starts_with("char_set_catcode")) {
                    shaping = true;
                    continue;
                }
                let c = match body.get(j) {
                    Some(y) if y.kind == AtomKind::Cs => {
                        y.name(text).bytes().next().filter(|_| y.end - y.start == 2)
                    }
                    Some(y) => match y.kind {
                        AtomKind::Char(c) if c.is_ascii() => u8::try_from(c).ok(),
                        _ => None,
                    },
                    None => None,
                };
                match c {
                    Some(c) if c < 128 && !lex::structural(c) => chars |= 1 << c,
                    _ => shaping = true,
                }
            }
            let Some(k) = first else { return (0, 0) };
            if !shaping {
                return (3, chars);
            }
            let opens = ["begingroup", "bgroup", "group_begin:"];
            // (a group opened first, here or closed by what it calls)
            if body[..k].iter().any(|x| is(x, &opens)) {
                (1, chars)
            } else {
                (2, chars)
            }
        };
        match name {
            "def" | "gdef" | "edef" | "xdef" | "protected@edef" | "protected@xdef" => {
                let Some(n) = cs_at(atoms, skip(atoms, a + 1)) else {
                    return;
                };
                let Some((args, open)) = param_text(text, atoms, n + 1) else {
                    return;
                };
                let target = atoms[n].name(text).to_owned();
                self.catcodes(&target, file, pos, body_catcodes(open));
                self.add(&target, file, pos, Shape::Macro(args));
            }
            "@namedef" => {
                let o = skip(atoms, a + 1);
                let Some(target) = group_text(atoms, o) else {
                    return;
                };
                let Some((args, open)) = param_text(text, atoms, atoms[o].mate + 1) else {
                    return;
                };
                self.catcodes(&target, file, pos, body_catcodes(open));
                self.add(&target, file, pos, Shape::Macro(args));
            }
            "let" => {
                let Some(n) = cs_at(atoms, skip(atoms, a + 1)) else {
                    return;
                };
                let mut t = skip(atoms, n + 1);
                if atoms.get(t).map(|x| x.kind) == Some(AtomKind::Char('=')) {
                    t = skip(atoms, t + 1);
                }
                let target = atoms[n].name(text).to_owned();
                let what = match atoms.get(t) {
                    Some(x) if x.kind == AtomKind::Cs => Shape::Alias(x.name(text).to_owned()),
                    Some(_) => Shape::Quantity('c'),
                    None => return,
                };
                self.add(&target, file, pos, what);
            }
            "newcommand" | "renewcommand" | "providecommand" | "DeclareRobustCommand" => {
                let mut j = skip(atoms, a + 1);
                if atoms.get(j).map(|x| x.kind) == Some(AtomKind::Char('*')) {
                    j = skip(atoms, j + 1);
                }
                let Some((target, after)) = name_arg(text, atoms, j) else {
                    return;
                };
                let (args, open) = newcommand_args(atoms, after);
                let Some(open) = open else { return };
                self.catcodes(&target, file, pos, body_catcodes(open));
                self.add(&target, file, pos, Shape::Macro(args));
            }
            "newenvironment" | "renewenvironment" => {
                let mut j = skip(atoms, a + 1);
                if atoms.get(j).map(|x| x.kind) == Some(AtomKind::Char('*')) {
                    j = skip(atoms, j + 1);
                }
                let Some(env) = group_text(atoms, j) else {
                    return;
                };
                let (args, open) = newcommand_args(atoms, atoms[j].mate + 1);
                let Some(open) = open else { return };
                if matches!(body_catcodes(open).0, 1 | 2) {
                    self.env_catcoders.insert(env.clone());
                }
                self.envs.entry(env).or_default().push(At {
                    file,
                    pos,
                    what: args,
                });
            }
            "NewDocumentCommand"
            | "RenewDocumentCommand"
            | "ProvideDocumentCommand"
            | "DeclareDocumentCommand"
            | "NewExpandableDocumentCommand"
            | "DeclareExpandableDocumentCommand" => {
                let j = skip(atoms, a + 1);
                let Some((target, after)) = name_arg(text, atoms, j) else {
                    return;
                };
                let s = skip(atoms, after);
                let Some(spec) = group_raw(text, atoms, s) else {
                    return;
                };
                let what = xparse(&spec).map_or_else(|e| Shape::Opaque(e.to_owned()), Shape::Macro);
                self.add(&target, file, pos, what);
            }
            "NewDocumentEnvironment"
            | "RenewDocumentEnvironment"
            | "DeclareDocumentEnvironment" => {
                let j = skip(atoms, a + 1);
                let Some(env) = group_text(atoms, j) else {
                    return;
                };
                let s = skip(atoms, atoms[j].mate + 1);
                let Some(spec) = group_raw(text, atoms, s) else {
                    return;
                };
                let args = xparse(&spec).unwrap_or_else(|e| vec![Arg::Pending(e.to_owned())]);
                self.envs.entry(env).or_default().push(At {
                    file,
                    pos,
                    what: args,
                });
            }
            "newtheorem" => {
                let j = skip(atoms, a + 1);
                let Some(env) = group_text(atoms, j) else {
                    return;
                };
                self.envs.entry(env).or_default().push(At {
                    file,
                    pos,
                    what: vec![Arg::Opt],
                });
            }
            "newif" => {
                let Some(n) = cs_at(atoms, skip(atoms, a + 1)) else {
                    return;
                };
                let target = atoms[n].name(text);
                if let Some(base) = target.strip_prefix("if") {
                    self.add(target, file, pos, Shape::Cond);
                    self.add(&format!("{base}true"), file, pos, Shape::Macro(Vec::new()));
                    self.add(&format!("{base}false"), file, pos, Shape::Macro(Vec::new()));
                }
            }
            "newcount" | "newdimen" | "newskip" | "newmuskip" | "newtoks" | "newlength"
            | "newbox" | "newread" | "newwrite" | "newlanguage" | "newinsert" | "newfam"
            | "countdef" | "dimendef" | "skipdef" | "muskipdef" | "toksdef" | "chardef"
            | "mathchardef" | "font" => {
                let mut j = skip(atoms, a + 1);
                if name == "newlength" && atoms.get(j).map(|x| x.kind) == Some(AtomKind::Open) {
                    j += 1;
                }
                let Some(n) = cs_at(atoms, j) else { return };
                let ty = match name {
                    "newcount" | "countdef" => 'i',
                    "newdimen" | "dimendef" => 'd',
                    "newskip" | "newmuskip" | "newlength" | "skipdef" | "muskipdef" => 'g',
                    "newtoks" | "toksdef" => 't',
                    _ => 'c',
                };
                self.add(atoms[n].name(text), file, pos, Shape::Quantity(ty));
            }
            "newcounter" => {
                let j = skip(atoms, a + 1);
                let Some(c) = group_text(atoms, j) else {
                    return;
                };
                self.add(&format!("c@{c}"), file, pos, Shape::Quantity('i'));
                self.add(&format!("the{c}"), file, pos, Shape::Macro(Vec::new()));
            }
            _ => {}
        }
    }
}

/// Past the spaces from `j`.
pub(crate) fn skip(atoms: &[Atom], mut j: usize) -> usize {
    while atoms
        .get(j)
        .is_some_and(|x| matches!(x.kind, AtomKind::Space))
    {
        j += 1;
    }
    j
}

fn cs_at(atoms: &[Atom], j: usize) -> Option<usize> {
    (atoms.get(j)?.kind == AtomKind::Cs).then_some(j)
}

/// A brace group's characters.
fn group_text(atoms: &[Atom], j: usize) -> Option<String> {
    if atoms.get(j)?.kind != AtomKind::Open {
        return None;
    }
    atoms::group_text(atoms, j)
}

/// A brace group's source text, inside its braces.
fn group_raw(text: &str, atoms: &[Atom], j: usize) -> Option<String> {
    let o = atoms.get(j)?;
    if o.kind != AtomKind::Open || o.mate == usize::MAX {
        return None;
    }
    Some(text[o.end..atoms[o.mate].start].to_owned())
}

/// A command's name as `\x` or `{\x}`: the name and the atom after it.
fn name_arg(text: &str, atoms: &[Atom], j: usize) -> Option<(String, usize)> {
    let x = atoms.get(j)?;
    match x.kind {
        AtomKind::Cs => Some((x.name(text).to_owned(), j + 1)),
        AtomKind::Open if x.mate != usize::MAX => {
            let n = cs_at(atoms, skip(atoms, j + 1))?;
            Some((atoms[n].name(text).to_owned(), x.mate + 1))
        }
        _ => None,
    }
}

/// `[n][default]{` from `j`: the arguments and the body's `{`.
fn newcommand_args(atoms: &[Atom], j: usize) -> (Vec<Arg>, Option<usize>) {
    let mut j = skip(atoms, j);
    let mut n = 0usize;
    let mut default = false;
    if atoms.get(j).map(|x| x.kind) == Some(AtomKind::Char('[')) {
        j += 1;
        while let Some(AtomKind::Char(c)) = atoms.get(j).map(|x| x.kind) {
            j += 1;
            if c == ']' {
                break;
            }
            if let Some(d) = c.to_digit(10) {
                n = d as usize;
            }
        }
        j = skip(atoms, j);
        if atoms.get(j).map(|x| x.kind) == Some(AtomKind::Char('[')) {
            default = true;
            let mut depth = 0usize;
            while let Some(x) = atoms.get(j) {
                j += 1;
                match x.kind {
                    AtomKind::Open => depth += 1,
                    AtomKind::Close => depth = depth.saturating_sub(1),
                    AtomKind::Char(']') if depth == 0 => break,
                    _ => {}
                }
            }
            j = skip(atoms, j);
        }
    }
    let mut args = Vec::new();
    if default && n > 0 {
        args.push(Arg::Opt);
        n -= 1;
    }
    args.extend(std::iter::repeat_n(Arg::M, n));
    let open = (atoms.get(j).map(|x| x.kind) == Some(AtomKind::Open)).then_some(j);
    (args, open)
}

#[allow(clippy::match_same_arms)]
/// `\def`'s parameter text from `j` up to its body's `{`: the arguments
/// and the `{`.
fn param_text(text: &str, atoms: &[Atom], j: usize) -> Option<(Vec<Arg>, usize)> {
    enum P {
        Param,
        Lit(Lit),
    }
    let mut ps = Vec::new();
    let mut k = j;
    let open = loop {
        let x = atoms.get(k)?;
        match x.kind {
            AtomKind::Open => break k,
            AtomKind::Close | AtomKind::Par => return None,
            // (`#{`: delimited by the body's brace; `##1`: a parameter of
            // a definition nested in another's body)
            AtomKind::Param(0) if atoms.get(k + 1).is_some_and(|y| y.kind == AtomKind::Open) => {
                ps.push(P::Lit(Lit::Char('{')));
            }
            AtomKind::Param(0)
                if matches!(atoms.get(k + 1).map(|y| y.kind), Some(AtomKind::Param(_))) =>
            {
                ps.push(P::Param);
                k += 1;
            }
            AtomKind::Param(0) => ps.push(P::Lit(Lit::Char('#'))),
            AtomKind::Param(_) => ps.push(P::Param),
            AtomKind::Cs => ps.push(P::Lit(Lit::Cs(x.name(text).to_owned()))),
            AtomKind::Char(c) => ps.push(P::Lit(Lit::Char(c))),
            AtomKind::Space => ps.push(P::Lit(Lit::Char(' '))),
            AtomKind::Verb => return None,
        }
        k += 1;
    };
    let mut args = Vec::new();
    let mut lits = Vec::new();
    let mut in_param = false;
    let flush = |args: &mut Vec<Arg>, lits: &mut Vec<Lit>, in_param: bool| {
        if in_param {
            args.push(match lits.as_slice() {
                [] => Arg::M,
                [Lit::Char('{')] => Arg::UntilOpen,
                _ => Arg::Delimited(std::mem::take(lits)),
            });
            lits.clear();
        } else if !lits.is_empty() {
            args.push(Arg::Literal(std::mem::take(lits)));
        }
    };
    for p in ps {
        match p {
            P::Param => {
                flush(&mut args, &mut lits, in_param);
                in_param = true;
            }
            P::Lit(l) => lits.push(l),
        }
    }
    flush(&mut args, &mut lits, in_param);
    Some((args, open))
}

/// xparse's argument spec as arguments.
#[allow(clippy::match_same_arms)]
fn xparse(spec: &str) -> Result<Vec<Arg>, &'static str> {
    let mut out = Vec::new();
    let mut cs = spec.chars().peekable();
    // (skips a `{…}` default)
    let skip_group = |cs: &mut std::iter::Peekable<std::str::Chars<'_>>| {
        while cs.peek() == Some(&' ') {
            cs.next();
        }
        if cs.peek() == Some(&'{') {
            let mut depth = 0;
            for c in cs.by_ref() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }
    };
    while let Some(c) = cs.next() {
        match c {
            ' ' | '+' | '!' | '\n' | '\t' => {}
            'm' => out.push(Arg::M),
            'o' => out.push(Arg::Opt),
            'O' => {
                skip_group(&mut cs);
                out.push(Arg::Opt);
            }
            's' => out.push(Arg::Star),
            't' => match cs.next() {
                Some('*') => out.push(Arg::Star),
                Some(c) if c != '\\' => out.push(Arg::OptChar(c)),
                _ => return Err("xparse t with a command token"),
            },
            'd' | 'D' => {
                let (a, b) = (cs.next(), cs.next());
                if c == 'D' {
                    skip_group(&mut cs);
                }
                if (a, b) == (Some('['), Some(']')) {
                    out.push(Arg::Opt);
                } else {
                    return Err("xparse delimited optional argument");
                }
            }
            'b' => {}
            'v' => return Err("xparse verbatim argument"),
            'r' | 'R' => return Err("xparse required delimited argument"),
            'g' | 'G' => return Err("xparse g argument"),
            'e' | 'E' => return Err("xparse embellishment"),
            'l' | 'u' => return Err("xparse l/u argument"),
            _ => return Err("xparse spec unreadable"),
        }
    }
    Ok(out)
}
