//! Definitions visible in text: what each defined name's uses consume,
//! read from its parameter text (`\def`), its argument count and default
//! (`\newcommand`), its argument spec (`\NewDocumentCommand`), or what it
//! was declared as (`\newcount`, `\newif`, `\let`).

use std::collections::{HashMap, HashSet};

use super::FileKind;
use super::atoms::{self, Atom, AtomKind};
use super::hints::{Closure, DefSet, Effects, effects};
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
    /// A pdfTeX command's keyword syntax, by the command's name
    /// (pdftex.web's `scan_action`, `scan_image`, `do_dest` …).
    Pdf(String),
    /// An array parameter (`\parshape`, `\interlinepenalties`): a
    /// count, then that many values of this type (`'P'`: dimen pairs).
    Array(char),
    /// A delimiter (tex.web §1160 `scan_delimiter`).
    Delim,
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
    /// With them, the catcodes it sets of characters its arguments name
    /// (`` \def\do#1{\catcode`#1=12 } ``): (parameter, catcode).
    catcoders: HashMap<String, Vec<At<Catcoder>>>,
    env_catcoders: HashSet<String>,
    /// Characters some `\catcode` makes active.
    active: u128,
    /// Per definition with a visible body, its text and what running it
    /// defines directly. The output routine set by `\output{…}` is kept
    /// under [`OUTPUT`].
    bodies: HashMap<String, Vec<At<Body>>>,
    /// The order the files load in (file → rank), when known: a later
    /// definition in another file then wins after it (load order).
    order: Option<HashMap<u32, u32>>,
    /// Names some `\\font` defines (their [`Shape::Quantity`] is `'c'`).
    fonts: HashSet<String>,
    /// Closures computed with these as the prelude ([`Closure`]): by
    /// name, the set and every name looked up for it. Shared by clones;
    /// dropped (a fresh table) by any change.
    pub(crate) closures: ClosureCache,
}

/// [`Definitions::closures`].
pub(crate) type ClosureCache =
    std::sync::Arc<std::sync::RwLock<HashMap<String, (DefSet, std::sync::Arc<Vec<String>>)>>>;

/// What a definition's body does to catcodes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Catcoder {
    /// The kind and characters ([`Definitions::changes_catcodes`]).
    pub kind: (u8, u128),
    /// Of those characters, the ones set to a literal catcode, in order:
    /// (character, catcode).
    pub set: Vec<(u8, u32)>,
    /// The catcodes set of the characters its arguments name:
    /// (parameter, catcode).
    pub of_args: Vec<(u8, u32)>,
    /// It ends a group its caller began (`\endgroup` with no
    /// `\begingroup` before it): the catcodes set in that group end there.
    pub ends_group: bool,
}

/// The key the `\output` routine's text is kept under.
pub const OUTPUT: &str = "\0output";

/// A definition's body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Body {
    pub text: String,
    pub effects: Effects,
}

pub(crate) enum Found<'a, T> {
    None,
    One(&'a T),
    Conflict,
}

fn find<'v, T: PartialEq>(
    v: Option<&'v Vec<At<T>>>,
    at: Option<(u32, usize)>,
    order: Option<&HashMap<u32, u32>>,
) -> Found<'v, T> {
    let Some(v) = v else { return Found::None };
    let Some((file, pos)) = at else {
        return v.last().map_or(Found::None, |d| Found::One(&d.what));
    };
    // (document order binds: the nearest definition before the use in its
    // file, or, when the files' load order is known, the last one loaded
    // before it; otherwise the definitions must agree)
    if let Some(d) = v
        .iter()
        .filter(|d| d.file == file && d.pos < pos)
        .max_by_key(|d| d.pos)
    {
        return Found::One(&d.what);
    }
    if let Some(o) = order {
        let rank = |f: u32| o.get(&f).copied().unwrap_or(u32::MAX);
        let me = (rank(file), pos);
        if let Some(d) = v
            .iter()
            .filter(|d| (rank(d.file), d.pos) < me && rank(d.file) != u32::MAX)
            .max_by_key(|d| (rank(d.file), d.pos))
        {
            return Found::One(&d.what);
        }
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
        find(self.macros.get(name), at, self.order.as_ref())
    }

    /// Where the definition of `name` bound at `at` stands, when one in
    /// its file (or, by load order, before it) binds.
    pub(crate) fn bound_at(&self, name: &str, at: (u32, usize)) -> Option<(u32, usize)> {
        let v = self.macros.get(name)?;
        let (file, pos) = at;
        if let Some(d) = v
            .iter()
            .filter(|d| d.file == file && d.pos < pos)
            .max_by_key(|d| d.pos)
        {
            return Some((d.file, d.pos));
        }
        let o = self.order.as_ref()?;
        let rank = |f: u32| o.get(&f).copied().unwrap_or(u32::MAX);
        v.iter()
            .filter(|d| (rank(d.file), d.pos) < (rank(file), pos) && rank(d.file) != u32::MAX)
            .max_by_key(|d| (rank(d.file), d.pos))
            .map(|d| (d.file, d.pos))
    }

    /// The body bound at `at` (as [`Definitions::find`] binds).
    pub(crate) fn find_body(&self, name: &str, at: Option<(u32, usize)>) -> Found<'_, Body> {
        find(self.bodies.get(name), at, self.order.as_ref())
    }

    /// Whether calling `name` ends a paragraph: its body's last command
    /// at brace depth 0 does (`\\par`, a blank line, a vertical command,
    /// or a macro that does, to `depth` levels).
    pub(crate) fn ends_paragraph(&self, name: &str, depth: u8) -> bool {
        if depth == 0 {
            return false;
        }
        let Found::One(body) = self.find_body(name, None) else {
            return false;
        };
        // (only a command that reads nothing after it: `\\par`, `\\end`,
        // or `\\vskip` of a register; a keyword or number scan would put
        // the next token back to run before the paragraph's end)
        match last_command(&body.text) {
            Some(Last::Blank) => true,
            Some(Last::Cs(n, prev)) => {
                matches!(n.as_str(), "par" | "end" | "tex_par:D")
                    || (prev.as_deref() == Some("vskip") && self.defines(&n))
                    || self.ends_paragraph(&n, depth - 1)
            }
            None => false,
        }
    }

    /// Every shape `name` is given.
    pub(crate) fn shapes(&self, name: &str) -> Option<Vec<&Shape>> {
        self.macros
            .get(name)
            .map(|v| v.iter().map(|d| &d.what).collect())
    }

    /// What each visible body of `name` defines directly.
    pub(crate) fn effects(&self, name: &str) -> Option<Vec<&Effects>> {
        self.bodies
            .get(name)
            .map(|v| v.iter().map(|d| &d.what.effects).collect())
    }

    /// Declares the order the files load in: `files[0]` first. A use then
    /// binds to the last definition loaded before it, in any file.
    pub fn set_load_order(&mut self, files: &[u32]) {
        self.closures = ClosureCache::default();
        self.order = Some(
            files
                .iter()
                .enumerate()
                .map(|(k, &f)| (f, u32::try_from(k).unwrap_or(u32::MAX)))
                .collect(),
        );
    }

    /// Whether a load order is declared.
    #[must_use]
    pub fn ordered(&self) -> bool {
        self.order.is_some()
    }

    /// The file of the definition of `name` bound at `at`, if it binds
    /// to one in another file by load order.
    pub(crate) fn bound_across_files(&self, name: &str, file: u32, pos: usize) -> bool {
        let Some(o) = &self.order else { return false };
        let Some(v) = self.macros.get(name) else {
            return false;
        };
        if v.iter().any(|d| d.file == file && d.pos < pos) {
            return false;
        }
        let rank = |f: u32| o.get(&f).copied().unwrap_or(u32::MAX);
        v.iter()
            .any(|d| rank(d.file) < rank(file) && rank(d.file) != u32::MAX)
    }

    /// What running the `\output` routine may define, from the last
    /// `\output{…}` assignment seen and every macro it calls (through the
    /// definitions of `self`, then `prelude`).
    #[must_use]
    pub fn output_defines(&self, prelude: &Definitions) -> Option<DefSet> {
        let mut c = Closure::new(self, prelude);
        self.bodies.contains_key(OUTPUT).then(|| c.call(OUTPUT))
    }

    /// What calling `name` may define (through `self`, then `prelude`).
    #[must_use]
    pub fn call_defines(&self, prelude: &Definitions, name: &str) -> DefSet {
        Closure::new(self, prelude).call(name)
    }

    /// Whether the document defines `name` anywhere.
    pub(crate) fn defines(&self, name: &str) -> bool {
        self.macros.contains_key(name)
    }

    /// Whether `name` is defined in `file` before `pos`.
    pub(crate) fn defined_before(&self, name: &str, file: u32, pos: usize) -> bool {
        self.macros.get(name).is_some_and(|v| {
            v.iter().any(|d| d.file == file && d.pos < pos)
                || self.order.as_ref().is_some_and(|o| {
                    let rank = |f: u32| o.get(&f).copied().unwrap_or(u32::MAX);
                    v.iter()
                        .any(|d| rank(d.file) < rank(file) && rank(d.file) != u32::MAX)
                })
        })
    }

    pub(crate) fn find_env(&self, name: &str, at: Option<(u32, usize)>) -> Found<'_, Vec<Arg>> {
        find(self.envs.get(name), at, self.order.as_ref())
    }

    /// The characters the document makes active somewhere.
    pub(crate) fn maybe_active(&self) -> u128 {
        self.active
    }

    /// The ASCII characters some `\catcode` line of the document makes
    /// active (bit `c` set), besides `~`.
    #[must_use]
    pub fn active_chars(&self) -> u128 {
        self.active
    }

    /// The one shape the text gives `name` (all its definitions agree),
    /// with `'f'` for a quantity some `\\font` makes.
    #[must_use]
    pub fn shape(&self, name: &str) -> Option<Shape> {
        let v = self.macros.get(name)?;
        let first = &v.first()?.what;
        if v.iter().any(|d| d.what != *first) {
            return None;
        }
        Some(match first {
            Shape::Quantity('c') if self.fonts.contains(name) => Shape::Quantity('f'),
            s => s.clone(),
        })
    }

    /// Whether the text defines `name` anywhere.
    #[must_use]
    pub fn is_defined(&self, name: &str) -> bool {
        self.defines(name)
    }

    /// Whether the definition of `name` bound at `at` changes catcodes
    /// (0 no, 1 for the arguments it reads, 2 for good).
    pub(crate) fn changes_catcodes(&self, name: &str, at: Option<(u32, usize)>) -> (u8, u128) {
        match find(self.catcoders.get(name), at, self.order.as_ref()) {
            Found::One(k) => k.kind,
            Found::None => (0, 0),
            Found::Conflict => {
                // (bound to one of several definitions: the worst)
                self.catcoders.get(name).map_or((0, 0), |v| {
                    // (one setting its arguments' catcodes: for good, as
                    // which is not known)
                    let k = v
                        .iter()
                        .map(|d| {
                            if d.what.of_args.is_empty() {
                                d.what.kind.0
                            } else {
                                2
                            }
                        })
                        .filter(|&k| k != 3)
                        .max()
                        .unwrap_or(0);
                    let m = v.iter().fold(0, |m, d| m | d.what.kind.1);
                    if k == 0 && m != 0 { (3, m) } else { (k, m) }
                })
            }
        }
    }

    /// The catcodes the definition of `name` bound at `at` sets of the
    /// characters its arguments name: (parameter, catcode).
    pub(crate) fn arg_catcodes(&self, name: &str, at: Option<(u32, usize)>) -> &[(u8, u32)] {
        match find(self.catcoders.get(name), at, self.order.as_ref()) {
            Found::One(k) => &k.of_args,
            _ => &[],
        }
    }

    /// The literal catcodes the definition of `name` bound at `at` sets:
    /// (character, catcode), in order.
    pub(crate) fn set_catcodes(&self, name: &str, at: Option<(u32, usize)>) -> &[(u8, u32)] {
        match find(self.catcoders.get(name), at, self.order.as_ref()) {
            Found::One(k) => &k.set,
            _ => &[],
        }
    }

    /// Whether the definition of `name` bound at `at` ends a group its
    /// caller began.
    pub(crate) fn ends_group(&self, name: &str, at: Option<(u32, usize)>) -> bool {
        matches!(
            find(self.catcoders.get(name), at, self.order.as_ref()),
            Found::One(k) if k.ends_group
        )
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

    /// Keeps the body whose `{` is `atoms[open]`.
    fn body(&mut self, name: &str, text: &str, atoms: &[Atom], open: usize, file: u32, pos: usize) {
        let close = atoms[open].mate;
        if close == usize::MAX {
            return;
        }
        let what = Body {
            text: text[atoms[open].end..atoms[close].start].to_owned(),
            effects: effects(text, atoms, open + 1, close, false),
        };
        let v = self.bodies.entry(name.to_owned()).or_default();
        if !v.iter().any(|d| d.file == file && d.what == what) || v.iter().all(|d| d.file != file) {
            v.push(At { file, pos, what });
        }
    }

    fn catcodes(&mut self, name: &str, file: u32, pos: usize, k: Catcoder) {
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
        self.closures = ClosureCache::default();
        // (read under the initial catcodes, a text without a backslash
        // (nor a `^^` that could make one, §355) has no control
        // sequence: nothing defined, nothing made active)
        if !text.contains('\\') && !text.contains("^^") {
            return;
        }
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
        match name {
            "def" | "gdef" | "edef" | "xdef" | "protected@edef" | "protected@xdef" => {
                let Some(n) = cs_at(atoms, skip(atoms, a + 1)) else {
                    return;
                };
                let Some((args, open)) = param_text(text, atoms, n + 1) else {
                    return;
                };
                let target = atoms[n].name(text).to_owned();
                let plain = args.iter().all(|a| *a == Arg::M);
                self.catcodes(&target, file, pos, body_catcodes(text, atoms, open, plain));
                self.add(&target, file, pos, Shape::Macro(args));
                self.body(&target, text, atoms, open, file, pos);
            }
            "@namedef" => {
                let o = skip(atoms, a + 1);
                let Some(target) = group_text(atoms, o) else {
                    return;
                };
                let Some((args, open)) = param_text(text, atoms, atoms[o].mate + 1) else {
                    return;
                };
                let plain = args.iter().all(|a| *a == Arg::M);
                self.catcodes(&target, file, pos, body_catcodes(text, atoms, open, plain));
                self.add(&target, file, pos, Shape::Macro(args));
                self.body(&target, text, atoms, open, file, pos);
            }
            "output" => {
                // (the output routine: `\output{…}` or `\output={…}`)
                let mut j = skip(atoms, a + 1);
                if atoms.get(j).map(|x| x.kind) == Some(AtomKind::Char('=')) {
                    j = skip(atoms, j + 1);
                }
                if atoms.get(j).map(|x| x.kind) == Some(AtomKind::Open) {
                    self.body(OUTPUT, text, atoms, j, file, pos);
                }
            }
            "cs_new:Npn"
            | "cs_new_protected:Npn"
            | "cs_new_nopar:Npn"
            | "cs_new_protected_nopar:Npn"
            | "cs_set:Npn"
            | "cs_set_protected:Npn"
            | "cs_gset:Npn"
            | "cs_gset_protected:Npn"
            | "cs_new:Npx"
            | "cs_new:Npe"
            | "cs_set_nopar:Npn"
            | "cs_gset_nopar:Npn" => {
                let Some(n) = cs_at(atoms, skip(atoms, a + 1)) else {
                    return;
                };
                let Some((_, open)) = param_text(text, atoms, n + 1) else {
                    return;
                };
                let target = atoms[n].name(text).to_owned();
                self.body(&target, text, atoms, open, file, pos);
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
                let plain = args.iter().all(|a| *a == Arg::M);
                self.catcodes(&target, file, pos, body_catcodes(text, atoms, open, plain));
                self.add(&target, file, pos, Shape::Macro(args));
                self.body(&target, text, atoms, open, file, pos);
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
                if matches!(body_catcodes(text, atoms, open, false).kind.0, 1 | 2) {
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
                if name == "font" {
                    self.fonts.insert(atoms[n].name(text).to_owned());
                }
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

/// What running the body whose `{` is `atoms[open]` does to catcodes.
/// `plain`: its parameters are undelimited, so a call's arguments can be
/// read as they come.
#[allow(clippy::too_many_lines)]
fn body_catcodes(text: &str, atoms: &[Atom], open: usize, plain: bool) -> Catcoder {
    let close = atoms[open].mate;
    if close == usize::MAX {
        return Catcoder::default();
    }
    let body = &atoms[open + 1..close];
    let is = |x: &Atom, names: &[&str]| x.kind == AtomKind::Cs && names.contains(&x.name(text));
    // (the characters changed, when each is literal and none shapes
    // groups; `\makeatletter` changes only whether `@` is a letter
    // and is not counted)
    let mut first = None;
    let mut chars = 0u128;
    let mut shaping = false;
    let mut of_args = Vec::new();
    let mut set = Vec::new();
    let mut unknown = 0u128;
    let (mut depth, mut ends_group) = (0usize, false);
    // (what is read but not run here: a token list's value, §1226, and
    // a definition's body, run when its macro is called: here, if it is)
    let mut idle = vec![false; body.len()];
    let mut nested: Vec<(&str, usize)> = Vec::new();
    for (k, x) in body.iter().enumerate() {
        if x.kind != AtomKind::Cs || idle[k] {
            continue;
        }
        let mut j = skip(body, k + 1);
        match x.name(text) {
            "def" | "gdef" | "edef" | "xdef" | "protected@edef" | "protected@xdef" => {
                if body.get(j).is_some_and(|y| is(y, &["noexpand"])) {
                    j = skip(body, j + 1);
                }
                let Some(t) = body.get(j).filter(|y| y.kind == AtomKind::Cs) else {
                    continue;
                };
                let Some(o) = (j + 1..body.len())
                    .take_while(|&o| !matches!(body[o].kind, AtomKind::Close | AtomKind::Par))
                    .find(|&o| body[o].kind == AtomKind::Open)
                else {
                    continue;
                };
                nested.push((t.name(text), open + 1 + o));
                j = o;
            }
            "toks@" | "toks" => {
                while matches!(
                    body.get(j).map(|y| y.kind),
                    Some(AtomKind::Char('0'..='9' | '='))
                ) {
                    j = skip(body, j + 1);
                }
                if body.get(j).map(|y| y.kind) != Some(AtomKind::Open) {
                    continue;
                }
            }
            _ => continue,
        }
        let m = body[j].mate;
        if m != usize::MAX {
            // (the definer and its name too: not a call)
            for f in &mut idle[k..=(m - open - 1).min(body.len() - 1)] {
                *f = true;
            }
        }
    }
    for (k, x) in body.iter().enumerate() {
        if x.kind != AtomKind::Cs || idle[k] {
            continue;
        }
        let n = x.name(text);
        match n {
            "begingroup" | "bgroup" | "group_begin:" => depth += 1,
            "endgroup" | "egroup" | "group_end:" if depth == 0 => ends_group = true,
            "endgroup" | "egroup" | "group_end:" => depth -= 1,
            _ => {}
        }
        if let Some(&(_, o)) = nested.iter().rev().find(|(t, _)| *t == n) {
            // (a macro the body defines, called in it)
            let inner = body_catcodes(text, atoms, o, false);
            ends_group |= inner.ends_group && depth == 0;
            if inner.kind.0 != 0 || !inner.of_args.is_empty() {
                first.get_or_insert(k);
                chars |= inner.kind.1;
                unknown |= inner.kind.1 & !inner.set.iter().fold(0, |m, &(c, _)| m | 1 << c);
                set.extend(inner.set);
                shaping |= inner.kind.0 != 3;
            }
            continue;
        }
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
            // (`\number` before an alphabetic constant: the same
            // number, §440)
            if body.get(j).is_some_and(|y| is(y, &["number"])) {
                j = skip(body, j + 1);
            }
            if body.get(j).map(|y| y.kind) != Some(AtomKind::Char('`')) {
                shaping = true;
                continue;
            }
            j += 1;
            if let Some(AtomKind::Param(p @ 1..=9)) = body.get(j).map(|y| y.kind) {
                // (`` `#1 ``: the character the argument names, §442;
                // set to a literal catcode)
                match catcode_value(text, body, j + 1) {
                    Some(v) if plain => of_args.push((p, v)),
                    _ => shaping = true,
                }
                continue;
            }
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
            Some(c) if c < 128 && !lex::structural(c) => {
                chars |= 1 << c;
                // (`\@makeother`: 12; `char_set_catcode_letter:N`: 11)
                let v = match n {
                    "catcode" | "tex_catcode:D" => catcode_value(text, body, j + 1),
                    "@makeother" | "char_set_catcode_other:N" => Some(12),
                    "char_set_catcode_letter:N" => Some(11),
                    _ => None,
                };
                match v {
                    Some(v) => set.push((c, v)),
                    None => unknown |= 1 << c,
                }
            }
            _ => shaping = true,
        }
    }
    set.retain(|&(c, _)| unknown >> c & 1 == 0);
    let done = |kind| Catcoder {
        kind,
        set,
        of_args,
        ends_group,
    };
    let Some(k) = first else {
        return done((0, 0));
    };
    if !shaping {
        return done(if chars == 0 { (0, 0) } else { (3, chars) });
    }
    let opens = ["begingroup", "bgroup", "group_begin:"];
    // (a group opened first, here or closed by what it calls)
    if body[..k].iter().any(|x| is(x, &opens)) {
        done((1, chars))
    } else {
        done((2, chars))
    }
}

/// A literal catcode from `j` in `atoms`: `=12`, ` 13`, `=\active`.
fn catcode_value(text: &str, atoms: &[Atom], j: usize) -> Option<u32> {
    let mut j = skip(atoms, j);
    if atoms.get(j)?.kind == AtomKind::Char('=') {
        j = skip(atoms, j + 1);
    }
    let x = atoms.get(j)?;
    if x.kind == AtomKind::Cs {
        return lex::named_value(x.name(text).as_bytes());
    }
    let mut v = None;
    while let Some(AtomKind::Char(c @ '0'..='9')) = atoms.get(j).map(|y| y.kind) {
        v = Some(v.unwrap_or(0) * 10 + c.to_digit(10)?);
        j += 1;
    }
    v
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

/// The last command of a body at brace depth 0.
#[derive(Debug, PartialEq, Eq)]
enum Last {
    /// The command, and the control sequence before it.
    Cs(String, Option<String>),
    Blank,
}

fn last_command(text: &str) -> Option<Last> {
    let b = text.as_bytes();
    let (mut i, mut depth, mut last) = (0usize, 0i32, None);
    while i < b.len() {
        match b[i] {
            b'\\' => {
                let s = i + 1;
                let mut e = s;
                while e < b.len() && (b[e].is_ascii_alphabetic() || b[e] == b'@') {
                    e += 1;
                }
                if e == s {
                    e = (s + 1).min(b.len());
                }
                if depth == 0 {
                    let prev = match last.take() {
                        Some(Last::Cs(p, _)) => Some(p),
                        _ => None,
                    };
                    last = Some(Last::Cs(text[s..e].to_owned(), prev));
                }
                i = e;
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    last = None;
                }
            }
            b'%' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'\n'
                if depth == 0
                    && text[i + 1..]
                        .trim_start_matches([' ', '\t'])
                        .starts_with('\n') =>
            {
                last = Some(Last::Blank);
            }
            b' ' | b'\t' | b'\n' => {}
            _ if depth == 0 => last = None,
            _ => {}
        }
        i += 1;
    }
    last
}
