//! What each item tells the engine, in the engine's terms: how much text
//! it claims and on what assumption, what it may define, what scope it
//! opens or closes, whether it starts or ends a paragraph by itself, and
//! what it reads. Pure data: the engine (defuse) is not a dependency.
//!
//! Field names are the trace's (`crates/tools/trace/src/fields.rs`), so
//! that a hint compares with a trace's writes as it is: `cs:\foo`,
//! `param:\tolerance`, `count:12`, `catcode:124`; a family whose index
//! is a value is named by its prefix (`count:`). A register allocated by
//! `\newcount\x` is `count:` (its number is a value, catalogue F10).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

use super::atoms::{Atom, AtomKind};
use super::defs::{Definitions, Shape, skip};
use super::table::{Entry, table};

/// The one fact a prediction rests on, as the engine checks it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Assumption {
    /// `cs` (a name without its escape) still has this meaning.
    MeaningIs { cs: String, expected: MeaningShape },
    /// Character `byte` still has catcode `cat`.
    CatcodeIs { byte: u8, cat: u8 },
}

/// The meaning a prediction assumes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MeaningShape {
    /// TeX's, e-TeX's or pdfTeX's primitive of that name.
    Primitive,
    /// The format's command (LaTeX's or plain's), as documented.
    Kernel,
    /// A macro with this parameter text (spec letters, as the pass reads
    /// it: `m` an undelimited argument, `o` an optional one, `d` a
    /// delimited one, `l` literal tokens, `{` delimited by a brace).
    Macro { params: String },
    /// `\let` to this name.
    Alias(String),
    /// A register or constant of type `i`, `d`, `g`, `t` or `c`.
    Register(char),
    /// A conditional (`\newif`).
    Conditional,
    /// An expl3 function, read from its signature.
    Expl3(String),
}

/// How much following text an item claims.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Claim {
    Known(usize),
    Predicted {
        n_tokens: usize,
        confirm: Assumption,
        also: Vec<Assumption>,
    },
    Pending {
        cause: &'static str,
    },
}

/// A field an item may define, and whether globally.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Def {
    pub field: String,
    pub global: bool,
}

/// What an item may define.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MayDefine {
    Nothing,
    Names(Vec<Def>),
    /// Fields whose names start with one of these.
    Prefixes(Vec<Def>),
    Any,
}

/// The kind of a scope (group codes, tex.web §269).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScopeKind {
    /// `{` … `}`.
    Simple,
    /// `\begingroup` … `\endgroup`.
    SemiSimple,
    /// `\begin{x}` … `\end{x}`.
    Env(String),
    /// `$` … `$` (M).
    Math,
    /// `$$` … `$$` (D).
    Display,
    /// `\hbox{` (H).
    HBox,
    /// `\vbox{`, `\vtop{`, `\vcenter{` (V).
    VBox,
}

/// A scope effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Open(ScopeKind),
    Close,
}

/// What an item does to scopes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScopeHint {
    None,
    Open(ScopeKind),
    Close,
    Predicted { effect: Effect, confirm: Assumption },
    Pending { cause: &'static str },
}

/// A paragraph begun or ended by the item itself (tex.web §1090, §1094).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Implicit {
    None,
    /// In vertical mode it backs up and starts a paragraph (§1090
    /// `new_graf`, or §1046 `insert_dollar_sign` for `^` and `_`).
    StartsParagraph,
    /// In horizontal mode it ends the paragraph (§1094 `end_graf`, or
    /// §1095 `head_for_vmode`: back up and insert `\par`).
    EndsParagraph,
}

/// Everything the pass predicts of one item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hints {
    pub claim: Claim,
    pub may_define: MayDefine,
    pub scope: ScopeHint,
    pub implicit: Implicit,
    /// Fields it reads literally (a hint, not a complete set).
    pub reads: Vec<String>,
    pub cause: Option<&'static str>,
    /// The macros whose bodies (as the text defines them) `may_define`
    /// relies on: right while each still has that meaning.
    pub relied: Vec<String>,
    /// The commands the item and the bodies it relies on call.
    pub calls: Vec<String>,
    /// Those of them reached outside every group the bodies open
    /// ([`DefSet::outer_calls`]).
    pub outer_calls: Vec<String>,
}

impl Hints {
    /// No claim, nothing defined, no scope, nothing implied.
    #[must_use]
    pub fn none() -> Hints {
        Hints {
            claim: Claim::Known(0),
            may_define: MayDefine::Nothing,
            scope: ScopeHint::None,
            implicit: Implicit::None,
            reads: Vec::new(),
            cause: None,
            relied: Vec::new(),
            calls: Vec::new(),
            outer_calls: Vec::new(),
        }
    }
}

/// Structure that is not a control sequence: `{`, `}`, `$`, `$$`, a
/// blank line, the first character of a run of text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mark {
    pub atom: usize,
    pub hints: Hints,
}

// ---------------------------------------------------------------------
// Field names.

pub(crate) fn cs(name: &str) -> String {
    format!("cs:\\{name}")
}

pub(crate) fn param(name: &str) -> String {
    format!("param:\\{name}")
}

/// The family a register primitive indexes, as the trace names it.
fn family(name: &str) -> Option<&'static str> {
    Some(match name {
        "count" => "count",
        "dimen" => "dimen",
        "skip" => "skip",
        "muskip" => "muskip",
        "toks" => "toks",
        "box" | "setbox" => "box",
        "catcode" => "catcode",
        "lccode" => "lccode",
        "uccode" => "uccode",
        "sfcode" => "sfcode",
        "mathcode" => "mathcode",
        "delcode" => "delcode",
        "textfont" => "textfont",
        "scriptfont" => "scriptfont",
        "scriptscriptfont" => "scriptscriptfont",
        _ => return None,
    })
}

/// The family of a register of type `t` (`\newcount`'s: `i`).
fn family_of_type(t: char) -> Option<&'static str> {
    Some(match t {
        'i' => "count",
        'd' => "dimen",
        'g' => "skip",
        't' => "toks",
        _ => return None,
    })
}

/// A literal integer at `p` (spaces first): digits, `'` octal, `"` hex,
/// or `` ` `` and a character.
pub(crate) fn literal_int(text: &str, atoms: &[Atom], p: usize) -> Option<u32> {
    let p = skip(atoms, p);
    let a = atoms.get(p)?;
    match a.kind {
        AtomKind::Char('`') => {
            let b = atoms.get(p + 1)?;
            match b.kind {
                AtomKind::Char(c) => Some(u32::from(c)),
                AtomKind::Cs => {
                    let n = b.name(text);
                    let mut cs = n.chars();
                    let c = cs.next()?;
                    cs.next().is_none().then_some(u32::from(c))
                }
                AtomKind::Open => Some(u32::from('{')),
                AtomKind::Close => Some(u32::from('}')),
                AtomKind::Space => Some(32),
                _ => None,
            }
        }
        // (the kernel's constants, [`CONSTANTS`]: plain.tex's and
        // ltplain.dtx's `\chardef`s and `\z@`)
        AtomKind::Cs => match a.name(text) {
            "z@" => Some(0),
            "@ne" => Some(1),
            "tw@" => Some(2),
            "thr@@" => Some(3),
            "sixt@@n" => Some(16),
            "@cclv" => Some(255),
            "@cclvi" => Some(256),
            "@m" => Some(1000),
            "@M" => Some(10000),
            "@MM" => Some(20000),
            _ => None,
        },
        AtomKind::Char(c) if c.is_ascii_digit() || c == '\'' || c == '"' => {
            let (radix, mut q) = match c {
                '\'' => (8, p + 1),
                '"' => (16, p + 1),
                _ => (10, p),
            };
            let mut v: u32 = 0;
            let mut any = false;
            while let Some(AtomKind::Char(d)) = atoms.get(q).map(|x| x.kind) {
                let Some(x) = d.to_digit(radix) else { break };
                if radix == 16 && d.is_ascii_lowercase() {
                    break;
                }
                v = v.checked_mul(radix)?.checked_add(x)?;
                any = true;
                q += 1;
            }
            any.then_some(v)
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------
// What a stretch of text may define.

/// What a stretch of text (a definition's body, an item's extent) may
/// define directly, and the macros it calls.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Effects {
    pub names: Vec<Def>,
    pub prefixes: Vec<Def>,
    /// Why the set cannot be bounded (empty: bounded).
    pub unbounded: Vec<String>,
    /// Names called (their bodies' effects are added by [`closure`]).
    pub calls: Vec<String>,
    /// Names whose meaning is assigned as a register (`\advance\x`).
    pub assigned: Vec<String>,
    /// The calls made outside every group the text opens (or after
    /// `\global`): a call inside one has its local definitions undone
    /// at the group's end (tex.web §283), so only its global ones count.
    pub outer_calls: Vec<String>,
}

const DEFINERS: &[&str] = &[
    "def",
    "gdef",
    "edef",
    "xdef",
    "protected@edef",
    "protected@xdef",
    "let",
    "futurelet",
    "chardef",
    "mathchardef",
    "countdef",
    "dimendef",
    "skipdef",
    "muskipdef",
    "toksdef",
    "font",
    "read",
    "readline",
];

const LATEX_DEFINERS: &[&str] = &[
    "newcommand",
    "renewcommand",
    "providecommand",
    "DeclareRobustCommand",
    "NewDocumentCommand",
    "RenewDocumentCommand",
    "ProvideDocumentCommand",
    "DeclareDocumentCommand",
    "NewExpandableDocumentCommand",
    "DeclareExpandableDocumentCommand",
];

/// `\newcount` and its kin, and the allocation register each advances
/// (plain.tex: `\count10`…`\count19`; LaTeX's `\newlanguage` too).
fn allocator(name: &str) -> Option<u32> {
    Some(match name {
        "newcount" => 10,
        "newdimen" | "newlength" => 11,
        "newskip" => 12,
        "newmuskip" => 13,
        "newbox" => 14,
        "newtoks" => 15,
        "newread" => 16,
        "newwrite" => 17,
        "newfam" => 18,
        "newlanguage" => 19,
        "newinsert" => 20,
        _ => return None,
    })
}

/// The globals a pdfTeX object command sets (pdftex.web's `pdf_last_…`).
fn pdf_last(name: &str) -> &'static [&'static str] {
    match name {
        "pdfobj" => &["pdflastobj"],
        "pdfxform" => &["pdflastxform"],
        "pdfximage" => &[
            "pdflastximage",
            "pdflastximagepages",
            "pdflastximagecolordepth",
        ],
        "pdfannot" => &["pdflastannot"],
        "pdfstartlink" => &["pdflastlink"],
        _ => &[],
    }
}

/// Commands that read in (and run) text the pass cannot see.
const LOADERS: &[&str] = &[
    "input",
    "@input",
    "@@input",
    "@iinput",
    "include",
    "InputIfFileExists",
    "usepackage",
    "RequirePackage",
    "RequirePackageWithOptions",
    "documentclass",
    "LoadClass",
    "LoadClassWithOptions",
    "scantokens",
    "tex_scantokens:D",
    "file_input:n",
    "includeonly",
    "@@end",
];

/// Commands after which the next token is not run.
const QUOTERS: &[&str] = &[
    "noexpand",
    "string",
    "meaning",
    "exp_not:N",
    "token_to_str:N",
    "token_to_meaning:N",
    "cs_meaning:N",
    "aftergroup",
    "afterassignment",
    "show",
    "@nameuse",
    "ifdefined",
    "if_cs_exist:N",
    "unless",
];

/// Commands that read a quantity named after them without assigning it.
const READERS: &[&str] = &[
    "the",
    // (the primitives that scan a value, §1057–§1061, §1103, §1072:
    // a quantity after them is read, not assigned)
    "penalty",
    "kern",
    "mkern",
    "hskip",
    "vskip",
    "mskip",
    "raise",
    "lower",
    "moveleft",
    "moveright",
    "char",
    "mathchar",
    "delimiter",
    "accent",
    "mathaccent",
    "radical",
    // (the kernel's commands that pass a value on to `\penalty` and
    // `\vskip`)
    "addpenalty",
    "addvspace",
    // (the insertion's number, §1099)
    "insert",
    "number",
    "romannumeral",
    "showthe",
    "ifnum",
    "ifdim",
    "ifodd",
    "ifcase",
    "ifvoid",
    "ifhbox",
    "ifvbox",
    "unhbox",
    "unvbox",
    "unhcopy",
    "unvcopy",
    "copy",
    "box",
    "wd",
    "ht",
    "dp",
    "tex_the:D",
    "int_value:w",
    "if_int_compare:w",
    "if_dim:w",
    "numexpr",
    "dimexpr",
    "glueexpr",
    "by",
];

/// An expl3 setter's target: `\tl_set:Nn \l_x_tl`, `\int_gincr:N`,
/// `\cs_new:Npn \foo`. Global when its verb is `g…`, `new` or `const`.
fn expl3_setter(name: &str) -> Option<bool> {
    let (base, sig) = name.rsplit_once(':')?;
    if !(sig.starts_with('N') || sig.starts_with('c')) {
        return None;
    }
    let verb = base.rsplit_once('_').map_or(base, |x| x.1);
    let verb2 = base.split('_').skip(1).collect::<Vec<_>>().join("_");
    let local = [
        "set",
        "put_left",
        "put_right",
        "incr",
        "decr",
        "zero",
        "clear",
        "add",
        "sub",
        "set_eq",
        "remove_all",
        "remove_once",
        "replace_all",
        "replace_once",
        "concat",
        "push",
        "pop",
        "set_true",
        "set_false",
    ];
    let global_only = ["new", "const", "new_eq", "new_protected", "new_nopar"];
    if base.starts_with("__") || base.starts_with("@@") {
        return None;
    }
    if global_only
        .iter()
        .any(|v| verb2 == *v || verb2.starts_with(&format!("{v}_")))
        || verb == "new"
        || verb == "const"
    {
        return Some(true);
    }
    for v in local {
        if verb2 == v || verb2.starts_with(&format!("{v}_")) {
            return Some(false);
        }
        if let Some(g) = verb2.strip_prefix('g')
            && (g == v || g.starts_with(&format!("{v}_")))
        {
            return Some(true);
        }
    }
    None
}

/// The literal characters after `\csname` at `p` up to the first token
/// that is not a character: the prefix, and whether `\endcsname` ended it.
fn csname_prefix(text: &str, atoms: &[Atom], mut p: usize) -> (String, bool, usize) {
    let mut s = String::new();
    while let Some(a) = atoms.get(p) {
        match a.kind {
            AtomKind::Char(c) => s.push(c),
            AtomKind::Space => s.push(' '),
            AtomKind::Cs if matches!(a.name(text), "endcsname" | "cs_end:") => {
                return (s, true, p + 1);
            }
            _ => return (s, false, p),
        }
        p += 1;
    }
    (s, false, p)
}

/// The literal characters of the group at `p`.
fn group_chars(atoms: &[Atom], p: usize) -> Option<String> {
    let p = skip(atoms, p);
    if atoms.get(p)?.kind != AtomKind::Open {
        return None;
    }
    super::atoms::group_text(atoms, p)
}

/// Past the group at `p` (spaces first), or `p` itself.
fn past_group(atoms: &[Atom], p: usize) -> usize {
    let q = skip(atoms, p);
    match atoms.get(q) {
        Some(a) if a.kind == AtomKind::Open && a.mate != usize::MAX => a.mate + 1,
        _ => p,
    }
}

/// Past a definition's parameter text and body from `p`.
fn past_body(atoms: &[Atom], mut p: usize) -> usize {
    while let Some(a) = atoms.get(p) {
        match a.kind {
            AtomKind::Open => {
                return if a.mate == usize::MAX {
                    atoms.len()
                } else {
                    a.mate + 1
                };
            }
            AtomKind::Close | AtomKind::Par => return p,
            _ => p += 1,
        }
    }
    p
}

/// What `atoms[lo..hi]` may define, read as code that runs (a nested
/// definition's body is skipped: it runs when that macro is called).
/// `global`: a `\global` prefix stands before it.
#[allow(clippy::too_many_lines)]
pub(crate) fn effects(text: &str, atoms: &[Atom], lo: usize, hi: usize, global: bool) -> Effects {
    let t = table();
    let mut e = Effects::default();
    let mut g = global;
    let mut reading = false;
    // (after `\expandafter`: the next token is expanded first, so a
    // quoter after it does not quote)
    let mut xa_next = false;
    let mut k = lo;
    let hi = hi.min(atoms.len());
    let name_at = |p: usize| -> Option<&str> {
        let p = skip(atoms, p);
        atoms
            .get(p)
            .filter(|a| a.kind == AtomKind::Cs)
            .map(|a| a.name(text))
    };
    // (the groups the text opens so far: braces and named groups)
    let mut depth: i64 = 0;
    let mut letted: Vec<&str> = Vec::new();
    let mut bodies: Vec<(&str, usize, usize)> = Vec::new();
    // (what the previous atom added, and whether it ran inside a group
    // without `\global`: its local definitions are undone at the
    // group's end, §283)
    let mut mark = (0, 0, 0, false);
    while k < hi {
        drop_local(&mut e, mark);
        mark = (
            e.names.len(),
            e.prefixes.len(),
            e.assigned.len(),
            depth > 0 && !g,
        );
        let a = atoms[k];
        match a.kind {
            AtomKind::Open => depth += 1,
            AtomKind::Close => depth -= 1,
            _ => {}
        }
        if a.kind != AtomKind::Cs {
            // (after `=` a quantity is a value, not assigned)
            if matches!(a.kind, AtomKind::Char('=' | '-' | '+')) {
                reading = reading || a.kind == AtomKind::Char('=') || rd_sign(atoms, k);
            } else if !matches!(a.kind, AtomKind::Space) {
                reading = false;
            }
            k += 1;
            continue;
        }
        // (expl3's `\tex_def:D` is `\def`)
        let n = a.name(text);
        let n = n
            .strip_prefix("tex_")
            .and_then(|x| x.strip_suffix(":D"))
            .filter(|x| t.primitive(x))
            .unwrap_or(n);
        match n {
            "bgroup" | "begingroup" | "group_begin:" => depth += 1,
            "egroup" | "endgroup" | "group_end:" => depth -= 1,
            _ => {}
        }
        let gl = std::mem::replace(&mut g, false);
        let rd = std::mem::replace(&mut reading, false);
        let xa = std::mem::replace(&mut xa_next, false);
        k += 1;
        // (the token `\\aftergroup` saves runs at the group's end, the
        // one `\\afterassignment` saves after the next assignment: a call)
        if matches!(n, "aftergroup" | "afterassignment")
            && let Some(x) = name_at(k)
            && !e.calls.iter().any(|c| c == x)
        {
            e.calls.push(x.to_owned());
            e.outer_calls.push(x.to_owned());
        }
        if matches!(n, "expandafter" | "exp_after:wN") {
            xa_next = true;
            continue;
        }
        if matches!(
            n,
            "global" | "long" | "outer" | "protected" | "tex_global:D"
        ) {
            g = gl || matches!(n, "global" | "tex_global:D");
            continue;
        }
        if QUOTERS.contains(&n) && !xa {
            k = skip(atoms, k) + 1;
            continue;
        }
        if matches!(
            n,
            "ifx" | "if_meaning:w" | "if" | "ifcat" | "if:w" | "if_charcode:w" | "if_catcode:w"
        ) {
            // (an operand is a token, or `\\noexpand` and the one it
            // quotes, §367)
            let operand = |p: usize| -> usize {
                let q = skip(atoms, p);
                match atoms.get(q) {
                    Some(x)
                        if x.kind == AtomKind::Cs
                            && matches!(x.name(text), "noexpand" | "exp_not:N") =>
                    {
                        skip(atoms, q + 1) + 1
                    }
                    _ => q + 1,
                }
            };
            k = operand(operand(k));
            continue;
        }
        if READERS.contains(&n) {
            // (the reader is a command too, with its own state:
            // `\\unhbox` voids the register it reads, §1110)
            if !rd && !e.calls.iter().any(|c| c == n) {
                e.calls.push(n.to_owned());
                if depth <= 0 || gl {
                    e.outer_calls.push(n.to_owned());
                }
            }
            reading = true;
            continue;
        }
        let target = |e: &mut Effects, p: usize, g: bool| -> usize {
            let p = skip(atoms, p);
            match atoms.get(p) {
                Some(x) if x.kind == AtomKind::Cs => {
                    let tn = x.name(text);
                    if matches!(tn, "csname" | "cs:w") {
                        let (pre, whole, q) = csname_prefix(text, atoms, p + 1);
                        let d = Def {
                            field: cs(&pre),
                            global: g,
                        };
                        if whole {
                            e.names.push(d);
                        } else if pre.is_empty() {
                            e.unbounded
                                .push("a definition whose name \\csname computes".to_owned());
                        } else {
                            e.prefixes.push(d);
                        }
                        q
                    } else if matches!(tn, "noexpand" | "expandafter" | "exp_not:N") {
                        // (`\def\noexpand\x` in an `\edef`: `\x`;
                        // `\expandafter\x\y`: a target computed)
                        if tn == "expandafter" {
                            e.unbounded
                                .push("a definition whose name \\expandafter computes".to_owned());
                            p + 1
                        } else {
                            match atoms.get(skip(atoms, p + 1)) {
                                Some(y) if y.kind == AtomKind::Cs => {
                                    e.names.push(Def {
                                        field: cs(y.name(text)),
                                        global: g,
                                    });
                                    skip(atoms, p + 1) + 1
                                }
                                _ => p + 1,
                            }
                        }
                    } else {
                        e.names.push(Def {
                            field: cs(tn),
                            global: g,
                        });
                        p + 1
                    }
                }
                Some(x) if matches!(x.kind, AtomKind::Char(_)) => {
                    // (an active character; `^^X` the character X xor 64,
                    // §352)
                    if let AtomKind::Char(c) = x.kind {
                        let caret = c == '^'
                            && atoms.get(p + 1).map(|y| y.kind) == Some(AtomKind::Char('^'));
                        let coded = caret
                            .then(|| atoms.get(p + 2))
                            .flatten()
                            .and_then(|y| match y.kind {
                                AtomKind::Char(d) if d.is_ascii() => {
                                    char::from_u32(u32::from(d) ^ 64)
                                }
                                _ => None,
                            });
                        match coded {
                            Some(d) => {
                                e.names.push(Def {
                                    field: format!("cs:{d}"),
                                    global: g,
                                });
                                return p + 3;
                            }
                            None => e.names.push(Def {
                                field: format!("cs:{c}"),
                                global: g,
                            }),
                        }
                    }
                    p + 1
                }
                Some(x) if matches!(x.kind, AtomKind::Param(_)) => {
                    e.unbounded
                        .push("a definition whose name is a parameter".to_owned());
                    p + 1
                }
                Some(x) if x.kind == AtomKind::Open => {
                    // (`\newcommand{\x}`)
                    if let Some(y) = atoms
                        .get(skip(atoms, p + 1))
                        .filter(|y| y.kind == AtomKind::Cs)
                    {
                        e.names.push(Def {
                            field: cs(y.name(text)),
                            global: g,
                        });
                    } else {
                        e.unbounded
                            .push("a definition target not literal".to_owned());
                    }
                    if x.mate == usize::MAX {
                        atoms.len()
                    } else {
                        x.mate + 1
                    }
                }
                _ => {
                    e.unbounded
                        .push("a definition target not literal".to_owned());
                    p + 1
                }
            }
        };
        let gg = gl || matches!(n, "gdef" | "xdef" | "protected@xdef");
        // (a name this text defines runs as defined here: `\\def\\next`)
        let local_name = atoms
            .get(skip(atoms, k))
            .filter(|x| x.kind == AtomKind::Cs)
            .map(|x| x.name(text));
        match n {
            "def" | "gdef" => {
                let p = target(&mut e, k, gg);
                k = past_body(atoms, p);
                if let Some(x) = local_name {
                    bodies.push((x, p, k));
                }
            }
            // (an expanded body runs as expansion here: `\csname` in it
            // defines; the loop reads it as code, an over-approximation)
            "edef" | "xdef" | "protected@edef" | "protected@xdef" => {
                let p = target(&mut e, k, gg);
                if let Some(x) = local_name {
                    bodies.push((x, p, past_body(atoms, p)));
                }
                k = p;
            }
            "let" | "futurelet" => {
                if let Some(x) = atoms.get(skip(atoms, k)).filter(|x| x.kind == AtomKind::Cs) {
                    letted.push(x.name(text));
                }
                let p = target(&mut e, k, gl);
                let mut q = skip(atoms, p);
                if atoms.get(q).map(|x| x.kind) == Some(AtomKind::Char('=')) {
                    q = skip(atoms, q + 1);
                }
                // (`\futurelet\a\b`: `\b` runs next; `\let\a\b`: `\a`
                // may run as `\b`, `\let\next\f@@t ... \next`)
                if let Some(x) = atoms.get(q)
                    && x.kind == AtomKind::Cs
                    && !e.calls.iter().any(|c| c == x.name(text))
                {
                    e.calls.push(x.name(text).to_owned());
                    e.outer_calls.push(x.name(text).to_owned());
                }
                k = q + 1;
            }
            _ if DEFINERS.contains(&n) => {
                if matches!(n, "read" | "readline") {
                    // (`\read n to \x`, tex.web §1225: the number and
                    // `to` may run past the item's own atom)
                    let mut q = k;
                    while q < atoms.len()
                        && !matches!(
                            atoms[q].kind,
                            AtomKind::Cs | AtomKind::Open | AtomKind::Close | AtomKind::Par
                        )
                    {
                        q += 1;
                    }
                    k = target(&mut e, q, gl);
                } else {
                    k = target(&mut e, k, gl);
                }
            }
            _ if LATEX_DEFINERS.contains(&n) => {
                let mut p = skip(atoms, k);
                if atoms.get(p).map(|x| x.kind) == Some(AtomKind::Char('*')) {
                    p += 1;
                }
                let before = e.names.len();
                p = target(&mut e, p, gl);
                if n.contains("Document") {
                    // (xparse also defines `\x code` and the like)
                    let moved: Vec<Def> = e.names.drain(before..).collect();
                    e.prefixes.extend(moved);
                    p = past_group(atoms, p);
                    k = past_group(atoms, p);
                } else {
                    k = past_body(atoms, p);
                }
            }
            "newenvironment"
            | "renewenvironment"
            | "NewDocumentEnvironment"
            | "RenewDocumentEnvironment"
            | "DeclareDocumentEnvironment" => {
                let mut p = skip(atoms, k);
                if atoms.get(p).map(|x| x.kind) == Some(AtomKind::Char('*')) {
                    p += 1;
                }
                if let Some(env) = group_chars(atoms, p) {
                    let d = |f: String| Def {
                        field: f,
                        global: gl,
                    };
                    if n.contains("Document") {
                        e.prefixes.push(d(cs(&env)));
                        e.prefixes.push(d(cs(&format!("end{env}"))));
                    } else {
                        e.names.push(d(cs(&env)));
                        e.names.push(d(cs(&format!("end{env}"))));
                    }
                } else {
                    e.unbounded
                        .push("an environment whose name is computed".to_owned());
                }
                // (name, optional arguments, two bodies: up to the
                // second body's end)
                let mut q = past_group(atoms, p);
                let mut bodies = 0;
                while q < hi && bodies < 2 {
                    q = skip(atoms, q);
                    match atoms.get(q).map(|x| x.kind) {
                        Some(AtomKind::Open) => {
                            q = past_group(atoms, q);
                            bodies += 1;
                        }
                        Some(AtomKind::Char('[')) => {
                            while q < hi && atoms[q].kind != AtomKind::Char(']') {
                                q = past_group(atoms, q).max(q + 1);
                            }
                            q += 1;
                        }
                        _ => break,
                    }
                }
                k = q;
            }
            // pdfTeX's allocation-order globals (pdfTeX §1542 `pdf_last_obj`,
            // §1546 `pdf_last_xform`, §1550 `pdf_last_ximage` and its pages
            // and colour depth, §1556 `pdf_last_annot`, §1558
            // `pdf_last_link`): the item's syntax is read by the table
            "pdfobj" | "pdfxform" | "pdfximage" | "pdfannot" | "pdfstartlink" => {
                for f in pdf_last(n) {
                    e.names.push(Def {
                        field: format!("misc:{f}"),
                        global: true,
                    });
                }
            }
            "newif" => {
                if let Some(tn) = name_at(k)
                    && let Some(base) = tn.strip_prefix("if")
                {
                    for f in [tn.to_owned(), format!("{base}true"), format!("{base}false")] {
                        e.names.push(Def {
                            field: cs(&f),
                            global: gl,
                        });
                    }
                }
                k = skip(atoms, k) + 1;
            }
            "newcounter" => {
                if let Some(c) = group_chars(atoms, k) {
                    for f in [
                        format!("c@{c}"),
                        format!("the{c}"),
                        format!("p@{c}"),
                        format!("cl@{c}"),
                    ] {
                        e.names.push(Def {
                            field: cs(&f),
                            global: true,
                        });
                    }
                    e.names.push(Def {
                        field: "count:10".to_owned(),
                        global: true,
                    });
                    // (`\@addtoreset` from the optional argument)
                    e.prefixes.push(Def {
                        field: cs("cl@"),
                        global: true,
                    });
                } else {
                    e.unbounded
                        .push("\\newcounter with a computed name".to_owned());
                }
                k = past_group(atoms, k);
            }
            "setcounter" | "addtocounter" | "stepcounter" | "refstepcounter" => {
                e.prefixes.push(Def {
                    field: "count:".to_owned(),
                    global: true,
                });
                if n == "refstepcounter" {
                    e.names.push(Def {
                        field: cs("@currentlabel"),
                        global: false,
                    });
                }
                k = past_group(atoms, k);
            }
            "setlength" | "addtolength" | "settowidth" | "settoheight" | "settodepth" => {
                let p = skip(atoms, k);
                let q = if atoms.get(p).map(|x| x.kind) == Some(AtomKind::Open) {
                    p + 1
                } else {
                    p
                };
                if let Some(x) = atoms.get(skip(atoms, q)).filter(|x| x.kind == AtomKind::Cs) {
                    e.assigned.push(x.name(text).to_owned());
                }
                k = past_group(atoms, k).max(k + 1);
            }
            "advance" | "multiply" | "divide" | "tex_advance:D" | "tex_multiply:D"
            | "tex_divide:D" => {
                if let Some(x) = atoms.get(skip(atoms, k)).filter(|x| x.kind == AtomKind::Cs) {
                    let xn = x.name(text);
                    if family(xn).is_none() {
                        e.assigned.push(xn.to_owned());
                        k = skip(atoms, k) + 1;
                    } else {
                        g = gl;
                    }
                }
            }
            "@namedef" | "@gnamedef" => {
                match group_chars(atoms, k) {
                    Some(nm) => e.names.push(Def {
                        field: cs(&nm),
                        global: gl || n == "@gnamedef",
                    }),
                    None => match atoms.get(skip(atoms, k)).map(|x| x.kind) {
                        Some(AtomKind::Open) => {
                            let o = skip(atoms, k);
                            let (pre, _, _) = csname_prefix(text, atoms, o + 1);
                            if pre.is_empty() {
                                e.unbounded
                                    .push("\\@namedef with a computed name".to_owned());
                            } else {
                                e.prefixes.push(Def {
                                    field: cs(&pre),
                                    global: gl || n == "@gnamedef",
                                });
                            }
                        }
                        _ => e
                            .unbounded
                            .push("\\@namedef with a computed name".to_owned()),
                    },
                }
                let p = past_group(atoms, k);
                k = past_body(atoms, p);
            }
            "csname" | "cs:w" => {
                // (`\csname` makes an undefined name `\relax`, locally,
                // §372)
                let (pre, whole, q) = csname_prefix(text, atoms, k);
                let d = Def {
                    field: cs(&pre),
                    global: false,
                };
                if whole {
                    e.names.push(d);
                } else if pre.is_empty() {
                    e.unbounded
                        .push("\\csname without a literal prefix".to_owned());
                } else {
                    e.prefixes.push(d);
                }
                k = q;
            }
            _ if LOADERS.contains(&n) => {
                e.unbounded
                    .push(format!("\\{n} (text the pass cannot see)"));
            }
            // l3kernel's patchers and primitive namers rewrite bodies
            _ if n.starts_with("__kernel_patch") || n.starts_with("__kernel_primitive") => {
                e.unbounded.push(format!("\\{n} (a kernel patcher)"));
            }
            "cs_undefine:N" | "cs_undefine:c" => {
                k = target(&mut e, k, true);
            }
            // `\cs_generate_variant:Nn \foo:nn { nx , nV }`: the variants
            "cs_generate_variant:Nn" | "cs_generate_variant:cn" => {
                let p = skip(atoms, k);
                let base: Option<String> = match atoms.get(p) {
                    Some(x) if x.kind == AtomKind::Cs && n.ends_with(":Nn") => {
                        Some(x.name(text).to_owned())
                    }
                    Some(x) if x.kind == AtomKind::Open => group_chars(atoms, p),
                    _ => None,
                };
                let q = if n.ends_with(":Nn") {
                    p + 1
                } else {
                    past_group(atoms, p)
                };
                let variants = group_chars(atoms, q);
                match (base, variants) {
                    (Some(b), Some(vs)) if b.contains(':') => {
                        let stem = b.rsplit_once(':').map_or(b.as_str(), |x| x.0);
                        for vsig in vs.split(',') {
                            let vsig: String =
                                vsig.chars().filter(|c| !c.is_whitespace()).collect();
                            if vsig.is_empty() {
                                continue;
                            }
                            e.names.push(Def {
                                field: cs(&format!("{stem}:{vsig}")),
                                global: true,
                            });
                        }
                    }
                    _ => e
                        .unbounded
                        .push(format!("\\{n} with a computed name or list")),
                }
                k = past_group(atoms, q).max(q);
            }
            // `\prg_new_conditional:Npnn \foo:n #1 { p , T , F , TF } {…}`:
            // `\foo_p:n`, `\foo:nT`, `\foo:nF`, `\foo:nTF`
            "prg_new_conditional:Npnn"
            | "prg_set_conditional:Npnn"
            | "prg_new_protected_conditional:Npnn"
            | "prg_set_protected_conditional:Npnn"
            | "prg_new_conditional:Nnn"
            | "prg_set_conditional:Nnn"
            | "prg_new_protected_conditional:Nnn"
            | "prg_set_protected_conditional:Nnn"
            | "prg_new_eq_conditional:NNn"
            | "prg_set_eq_conditional:NNn" => {
                let p = skip(atoms, k);
                match atoms.get(p) {
                    Some(x) if x.kind == AtomKind::Cs && x.name(text).contains(':') => {
                        let full = x.name(text);
                        let (stem, sig) = full.rsplit_once(':').unwrap_or((full, ""));
                        for f in [
                            format!("{stem}_p:{sig}"),
                            format!("{stem}:{sig}T"),
                            format!("{stem}:{sig}F"),
                            format!("{stem}:{sig}TF"),
                        ] {
                            e.names.push(Def {
                                field: cs(&f),
                                global: true,
                            });
                        }
                    }
                    _ => e.unbounded.push(format!("\\{n} with a computed name")),
                }
                let mut q = p + 1;
                if n.ends_with(":NNn") {
                    q = skip(atoms, q) + 1;
                    q = past_group(atoms, q);
                } else if n.contains(":Np") {
                    q = past_body(atoms, q);
                    q = past_group(atoms, q);
                } else {
                    q = past_group(atoms, q);
                    q = past_group(atoms, q);
                }
                k = q.max(k);
            }
            _ => {
                if let Some(f) = allocator(n) {
                    let p = if n == "newlength" {
                        let p = skip(atoms, k);
                        if atoms.get(p).map(|x| x.kind) == Some(AtomKind::Open) {
                            p + 1
                        } else {
                            p
                        }
                    } else {
                        k
                    };
                    let q = target(&mut e, p, true);
                    e.names.push(Def {
                        field: format!("count:{f}"),
                        global: true,
                    });
                    k = if n == "newlength" {
                        past_group(atoms, k).max(q)
                    } else {
                        q
                    };
                    continue;
                }
                if let Some(fam) = family(n) {
                    if rd {
                        continue;
                    }
                    match literal_int(text, atoms, k) {
                        Some(v) => e.names.push(Def {
                            field: format!("{fam}:{v}"),
                            global: gl,
                        }),
                        None => {
                            e.prefixes.push(Def {
                                field: format!("{fam}:"),
                                global: gl,
                            });
                        }
                    }
                    continue;
                }
                if let Some(Entry::Quantity(_, None)) = t.commands.get(n) {
                    if !rd && !CONSTANTS.contains(&n) && assigned_here(atoms, k) {
                        e.names.push(Def {
                            field: param(n),
                            global: gl,
                        });
                        // (what follows is its value)
                        reading = true;
                    }
                    continue;
                }
                if let Some(gx) = expl3_setter(n) {
                    match atoms.get(skip(atoms, k)) {
                        Some(x) if x.kind == AtomKind::Cs => {
                            e.names.push(Def {
                                field: cs(x.name(text)),
                                global: gx || gl,
                            });
                            k = skip(atoms, k) + 1;
                        }
                        Some(x)
                            if x.kind == AtomKind::Open && n.ends_with(":c")
                                || n.contains(":c") =>
                        {
                            match group_chars(atoms, k) {
                                Some(nm) => e.names.push(Def {
                                    field: cs(&nm),
                                    global: gx || gl,
                                }),
                                None => e.unbounded.push(format!("\\{n} with a computed name")),
                            }
                            k = past_group(atoms, k);
                        }
                        _ => e.unbounded.push(format!("\\{n} with a computed target")),
                    }
                    // (the rest of its arguments are values or a body,
                    // not code that runs here)
                    let sig = n.rsplit_once(':').map_or("", |x| x.1);
                    for ch in sig.chars().skip(1) {
                        match ch {
                            'N' => k = skip(atoms, k) + 1,
                            'p' => {
                                k = past_body(atoms, k);
                                break;
                            }
                            // (expanded, or unknown: read as code)
                            'x' | 'e' | 'f' | 'o' | 'V' | 'v' | 'w' | 'D' => break,
                            _ => {
                                let q = past_group(atoms, k);
                                if q == k {
                                    break;
                                }
                                k = q;
                            }
                        }
                    }
                    continue;
                }
                // (a name this text `\let` runs as what it was let to,
                // a call the `\let` recorded, not as its definitions
                // elsewhere: `\let\next\f@@t ... \next`)
                if rd || letted.contains(&n) {
                    continue;
                }
                if let Some(&(_, b0, b1)) = bodies.iter().rev().find(|b| b.0 == n) {
                    // (its body as this text defined it runs here)
                    let x = effects(text, atoms, b0, b1, gl);
                    e.names.extend(x.names);
                    e.prefixes.extend(x.prefixes);
                    e.assigned.extend(x.assigned);
                    e.unbounded.extend(x.unbounded);
                    for c in x.calls {
                        if (depth <= 0 || gl)
                            && x.outer_calls.contains(&c)
                            && !e.outer_calls.contains(&c)
                        {
                            e.outer_calls.push(c.clone());
                        }
                        if !e.calls.contains(&c) {
                            e.calls.push(c);
                        }
                    }
                    continue;
                }
                e.calls.push(n.to_owned());
                if depth <= 0 || gl {
                    e.outer_calls.push(n.to_owned());
                }
            }
        }
    }
    drop_local(&mut e, mark);
    e
}

/// Drops the local definitions and assignments added since `mark`
/// when they ran inside a group the text opens (§283).
fn drop_local(e: &mut Effects, (names, prefixes, assigned, inner): (usize, usize, usize, bool)) {
    if !inner {
        return;
    }
    let keep = |v: &mut Vec<Def>, from: usize| {
        let mut i = 0;
        v.retain(|d| {
            i += 1;
            i <= from || d.global
        });
    };
    keep(&mut e.names, names);
    keep(&mut e.prefixes, prefixes);
    e.assigned.truncate(assigned);
}

/// The kernel's constants (plain.tex's `\chardef`s and `\dimen`s that
/// nothing assigns: §"constants" of plain.tex, ltplain.dtx).
const CONSTANTS: &[&str] = &[
    "@ne", "tw@", "thr@@", "sixt@@n", "@cclv", "@cclvi", "@m", "@M", "@MM", "@xxxii", "m@ne", "z@",
    "p@", "maxdimen", "@Mi", "@Mii", "@Miii", "@Miv",
];

/// Whether the sign at `k` follows a relation or `=` (a value goes on).
fn rd_sign(atoms: &[Atom], k: usize) -> bool {
    k > 0
        && matches!(
            atoms[k - 1].kind,
            AtomKind::Char('=' | '<' | '>' | '-' | '+')
        )
}

/// Whether a quantity named just before `p` is assigned there: `=` or a
/// value follows.
fn assigned_here(atoms: &[Atom], p: usize) -> bool {
    match atoms.get(skip(atoms, p)).map(|x| x.kind) {
        Some(AtomKind::Char(c)) => c == '=' || c.is_ascii_digit() || "+-.,'\"`".contains(c),
        Some(AtomKind::Cs | AtomKind::Open) => true,
        _ => false,
    }
}

/// The closure of effects over the macros called: what running the
/// effects' text may define.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DefSet {
    pub names: BTreeMap<String, bool>,
    pub prefixes: BTreeMap<String, bool>,
    /// What makes it unbounded, with where (`\caller: reason`).
    pub unbounded: Vec<String>,
    /// The macros whose bodies (as the text defines them) the set
    /// relies on: right while each still has that meaning.
    pub relied: Vec<String>,
    /// Every command the text and the bodies it relies on call (the
    /// engine's state each may touch is the client's to say).
    pub calls: Vec<String>,
    /// The calls reached outside every group the texts open (or after
    /// `\global`); the others run inside one, their local definitions
    /// undone at its end.
    pub outer_calls: Vec<String>,
}

/// The hasher of [`Acc`]'s sets: a multiply-rotate over the bytes, eight
/// at a time (names, no adversary).
#[derive(Default, Clone, Copy)]
pub(crate) struct Fx(u64);

impl Fx {
    fn mix(&mut self, x: u64) {
        self.0 = (self.0.rotate_left(5) ^ x).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}

impl std::hash::Hasher for Fx {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        let (words, rest) = bytes.as_chunks::<8>();
        for w in words {
            self.mix(u64::from_le_bytes(*w));
        }
        for &b in rest {
            self.mix(u64::from(b));
        }
    }
}

/// A set of names hashed with [`Fx`].
type NameSet = HashSet<String, std::hash::BuildHasherDefault<Fx>>;

/// [`DefSet::add`] of many sets in turn into one: each list's entries
/// kept in a set as they come, made once, not once per set added (a
/// closure adding the sets of many calls costs their sizes, not their
/// sizes times its own).
struct Acc {
    s: DefSet,
    /// The entries of `relied`, `calls`, `outer_calls`, `unbounded`.
    seen: [NameSet; 4],
}

impl Acc {
    fn new(s: DefSet) -> Acc {
        let set = |l: &[String]| l.iter().cloned().collect::<NameSet>();
        let seen = [
            set(&s.relied),
            set(&s.calls),
            set(&s.outer_calls),
            set(&s.unbounded),
        ];
        Acc { s, seen }
    }

    /// `x` added to `to` unless there already.
    fn push(to: &mut Vec<String>, seen: &mut NameSet, x: &str) {
        if !seen.contains(x) {
            seen.insert(x.to_owned());
            to.push(x.to_owned());
        }
    }

    /// [`DefSet::add`] of `o`; `outer: false`: of `o` run inside a group
    /// (no outer calls, its global definitions alone).
    fn add(&mut self, o: &DefSet, outer: bool) {
        let [relied, calls, outer_calls, unbounded] = &mut self.seen;
        for x in &o.relied {
            Acc::push(&mut self.s.relied, relied, x);
        }
        for x in &o.calls {
            Acc::push(&mut self.s.calls, calls, x);
        }
        if outer {
            for x in &o.outer_calls {
                Acc::push(&mut self.s.outer_calls, outer_calls, x);
            }
        }
        for (k, v) in &o.names {
            if outer || *v {
                *self.s.names.entry(k.clone()).or_default() |= v;
            }
        }
        for (k, v) in &o.prefixes {
            if outer || *v {
                *self.s.prefixes.entry(k.clone()).or_default() |= v;
            }
        }
        for x in &o.unbounded {
            Acc::push(&mut self.s.unbounded, unbounded, x);
        }
    }
}

impl DefSet {
    fn add(&mut self, o: &DefSet) {
        // (each list kept in its order, each entry once: a set of what
        // it holds made once per list, not a search per entry; a set
        // added often, a macro called many times, costs its size)
        fn merge(to: &mut Vec<String>, from: &[String]) {
            if from.is_empty() {
                return;
            }
            let new: Vec<String> = {
                let mut seen: HashSet<&str> = to.iter().map(String::as_str).collect();
                from.iter()
                    .filter(|x| seen.insert(x.as_str()))
                    .cloned()
                    .collect()
            };
            to.extend(new);
        }
        merge(&mut self.relied, &o.relied);
        merge(&mut self.calls, &o.calls);
        merge(&mut self.outer_calls, &o.outer_calls);
        for (k, v) in &o.names {
            *self.names.entry(k.clone()).or_default() |= v;
        }
        for (k, v) in &o.prefixes {
            *self.prefixes.entry(k.clone()).or_default() |= v;
        }
        merge(&mut self.unbounded, &o.unbounded);
    }

    /// As a [`MayDefine`].
    #[must_use]
    pub fn may_define(&self) -> MayDefine {
        if !self.unbounded.is_empty() {
            return MayDefine::Any;
        }
        let defs = |m: &BTreeMap<String, bool>| {
            m.iter()
                .map(|(f, g)| Def {
                    field: f.clone(),
                    global: *g,
                })
                .collect::<Vec<_>>()
        };
        if !self.prefixes.is_empty() {
            let mut v = defs(&self.prefixes);
            v.extend(defs(&self.names));
            v.sort();
            return MayDefine::Prefixes(v);
        }
        if self.names.is_empty() {
            MayDefine::Nothing
        } else {
            MayDefine::Names(defs(&self.names))
        }
    }
}

/// Where the closure looks names up: the document's definitions, then
/// the kernel's.
pub(crate) struct Closure<'a> {
    pub document: &'a Definitions,
    pub prelude: &'a Definitions,
    /// By name: the set, the names looked up for it, and whether a
    /// recursion cut it short (then it depends on the order of calls).
    memo: HashMap<String, (DefSet, std::sync::Arc<Vec<String>>, bool)>,
    /// By name, a call's whole set at the top ([`Closure::call_top`]):
    /// the same for every call of the name in this closure's life (its
    /// definitions do not change).
    tops: HashMap<String, std::sync::Arc<DefSet>>,
    busy: HashSet<String>,
    /// The call being computed: the names looked up so far, whether a
    /// recursion cut it short.
    frame: (Vec<String>, bool),
}

impl<'a> Closure<'a> {
    pub(crate) fn new(document: &'a Definitions, prelude: &'a Definitions) -> Closure<'a> {
        Closure {
            document,
            prelude,
            memo: HashMap::new(),
            tops: HashMap::new(),
            busy: HashSet::new(),
            frame: (Vec::new(), false),
        }
    }

    /// Whether the document defines any of `names` (a set the prelude's
    /// cache holds is then not this document's).
    fn document_defines(&self, names: &[String]) -> bool {
        names
            .iter()
            .any(|n| self.document.effects(n).is_some() || self.document.shapes(n).is_some())
    }

    /// What running `e` may define.
    pub(crate) fn of(&mut self, e: &Effects) -> DefSet {
        let mut s = DefSet::default();
        for d in &e.names {
            *s.names.entry(d.field.clone()).or_default() |= d.global;
        }
        for d in &e.prefixes {
            *s.prefixes.entry(d.field.clone()).or_default() |= d.global;
        }
        s.unbounded.extend(e.unbounded.iter().cloned());
        let mut acc = Acc::new(s);
        for a in &e.assigned {
            let x = self.assigned(a);
            acc.add(&x, true);
        }
        for c in &e.calls {
            Acc::push(&mut acc.s.calls, &mut acc.seen[1], c);
            let x = self.call(c);
            if e.outer_calls.contains(c) {
                Acc::push(&mut acc.s.outer_calls, &mut acc.seen[2], c);
                acc.add(&x, true);
            } else {
                // (inside a group: what it calls runs there too, and its
                // local definitions are undone at the group's end)
                acc.add(&x, false);
            }
        }
        acc.s
    }

    /// What assigning the quantity `name` defines.
    pub(crate) fn assigned(&mut self, name: &str) -> DefSet {
        self.frame.0.push(name.to_owned());
        let mut s = DefSet::default();
        if let Some(Entry::Quantity(_, None)) = table().commands.get(name) {
            s.names.insert(param(name), false);
            return s;
        }
        let shapes = self
            .document
            .shapes(name)
            .or_else(|| self.prelude.shapes(name));
        match shapes.and_then(|v| v.last().copied()) {
            Some(Shape::Quantity(t)) => match family_of_type(*t) {
                // (a `\countdef` that gave its number: that register)
                Some(f) => match self.document.shapes(name).map_or_else(
                    || self.prelude.register(name),
                    |_| self.document.register(name),
                ) {
                    Some(r) => {
                        s.names.insert(format!("{f}:{r}"), false);
                    }
                    None => {
                        s.prefixes.insert(format!("{f}:"), false);
                    }
                },
                None => s
                    .unbounded
                    .push(format!("\\{name}: a quantity of unknown kind")),
            },
            _ => s
                .unbounded
                .push(format!("\\{name}: assigned, not a register")),
        }
        s
    }

    /// What calling `name` may define.
    ///
    /// A set that looked up no name the document defines and met no
    /// recursion is the prelude's alone: it is kept in the prelude's
    /// cache ([`Definitions::closures`]) for every text analysed with it,
    /// and taken from there while the document defines none of the
    /// names it looked up and none of them is being computed.
    pub(crate) fn call(&mut self, name: &str) -> std::sync::Arc<DefSet> {
        // (a call at the top, no call being computed: its whole set, as
        // a fresh closure computes it, a function of the name and the
        // definitions alone, kept in the prelude's cache under its own
        // key even where a recursion cut it: a text's every call of a
        // recursive macro is then one lookup, not the walk again; and
        // kept for this closure's life, [`Closure::tops`])
        if self.busy.is_empty() {
            if let Some(s) = self.tops.get(name) {
                return s.clone();
            }
            let s = self.call_top(name);
            self.tops.insert(name.to_owned(), s.clone());
            return s;
        }
        std::sync::Arc::new(self.call_inner(name))
    }

    /// [`Closure::call`] at the top: from the prelude's cache of whole
    /// sets, else computed by a fresh closure (no memo of this one's
    /// read: what a recursion cut short inside another call is not
    /// this call's set).
    fn call_top(&mut self, name: &str) -> std::sync::Arc<DefSet> {
        let key = format!("\u{1}{name}");
        let cached = self.prelude.closure(&key);
        // (the names a top call looked up are no other call's: no frame
        // takes them)
        if let Some((s, names)) = cached
            && !self.document_defines(&names)
        {
            return s;
        }
        let mut fresh = Closure::new(self.document, self.prelude);
        let s = std::sync::Arc::new(fresh.call_inner(name));
        let (mut names, _) = std::mem::take(&mut fresh.frame);
        names.sort_unstable();
        names.dedup();
        let names = std::sync::Arc::new(names);
        if !self.document_defines(&names) {
            self.prelude.keep_closure(key, s.clone(), names);
        }
        s
    }

    /// [`Closure::call`] inside another call (its memo and recursion
    /// guard), or computing a top call afresh.
    fn call_inner(&mut self, name: &str) -> DefSet {
        if let Some((s, names, cut)) = self.memo.get(name) {
            let s = s.clone();
            self.frame.0.extend(names.iter().cloned());
            self.frame.1 |= *cut;
            return s;
        }
        if self.busy.contains(name) {
            // (a recursion: its effects are those being collected)
            self.frame.1 = true;
            return DefSet::default();
        }
        let cached = self.prelude.closure(name);
        if let Some((s, names)) = cached
            && !names.iter().any(|n| self.busy.contains(n))
            && !self.document_defines(&names)
        {
            let s = (*s).clone();
            self.frame.0.extend(names.iter().cloned());
            self.memo.insert(name.to_owned(), (s.clone(), names, false));
            return s;
        }
        self.busy.insert(name.to_owned());
        let outer = std::mem::replace(&mut self.frame, (vec![name.to_owned()], false));
        let s = self.compute(name);
        let (mut names, cut) = std::mem::replace(&mut self.frame, outer);
        self.busy.remove(name);
        names.sort_unstable();
        names.dedup();
        let names = std::sync::Arc::new(names);
        if !cut && !self.document_defines(&names) {
            self.prelude.keep_closure(
                name.to_owned(),
                std::sync::Arc::new(s.clone()),
                names.clone(),
            );
        }
        self.frame.0.extend(names.iter().cloned());
        self.frame.1 |= cut;
        self.memo.insert(name.to_owned(), (s.clone(), names, cut));
        s
    }

    fn compute(&mut self, name: &str) -> DefSet {
        let mut s = DefSet::default();
        let doc = self.document.effects(name);
        let pre = self.prelude.effects(name);
        let bodies: Vec<Effects> = match (doc, pre) {
            (Some(d), _) => d.into_iter().cloned().collect(),
            (None, Some(p)) => p.last().into_iter().copied().cloned().collect(),
            (None, None) => Vec::new(),
        };
        if !bodies.is_empty() {
            s.relied.push(name.to_owned());
            for b in bodies {
                let x = self.of(&b);
                s.add(&x);
            }
            return s;
        }
        let shapes = self
            .document
            .shapes(name)
            .or_else(|| self.prelude.shapes(name));
        if let Some(v) = shapes {
            s.relied.push(name.to_owned());
            for sh in v {
                match sh {
                    Shape::Alias(t) => {
                        let t = t.clone();
                        let x = self.call(&t);
                        s.add(&x);
                    }
                    Shape::Quantity(_) | Shape::Cond => {}
                    Shape::Macro(_) => {
                        s.unbounded.push(format!("\\{name}: body not read"));
                    }
                    Shape::Opaque(e) => s.unbounded.push(format!("\\{name}: {e}")),
                }
            }
            return s;
        }
        if table().primitive(name) || is_control_symbol(name) {
            // (a primitive: it defines nothing but what its own syntax
            // names, which `effects` reads at the call)
            return s;
        }
        if let Some(Entry::Command(_)) = expl3_primitive(name) {
            return s;
        }
        if table().commands.contains_key(name) {
            // (a format's command whose syntax the table gives but whose
            // body the text did not: what it defines is not read)
            s.unbounded
                .push(format!("\\{name}: a kernel macro whose body was not read"));
            return s;
        }
        s.unbounded.push(format!("\\{name} defined elsewhere"));
        s
    }
}

fn is_control_symbol(name: &str) -> bool {
    let mut c = name.chars();
    matches!((c.next(), c.next()), (Some(x), None) if !x.is_alphabetic())
}

/// An expl3 `\tex_…:D` name's primitive.
fn expl3_primitive(name: &str) -> Option<&'static Entry> {
    let p = name.strip_prefix("tex_")?.strip_suffix(":D")?;
    table().commands.get(p)
}

/// The spec letters of a macro's arguments, for [`MeaningShape::Macro`].
pub(crate) fn params_of(args: &[super::defs::Arg]) -> String {
    use super::defs::Arg;
    args.iter()
        .map(|a| match a {
            Arg::M => 'm',
            Arg::Opt => 'o',
            Arg::Star => 's',
            Arg::Tok => 'N',
            Arg::Delimited(_) => 'd',
            Arg::Literal(_) => 'l',
            Arg::UntilOpen => '{',
            _ => '?',
        })
        .collect()
}

// ---------------------------------------------------------------------
// Paragraphs (tex.web §1090, §1094).

/// Commands that start a paragraph in vertical mode: §1090
/// (`vmode+start_par`, `letter`, `other_char`, `char_num`, `char_given`,
/// `math_shift`, `un_hbox`, `vrule`, `accent`, `discretionary`, `hskip`,
/// `valign`, `ex_space`, `no_boundary`). `\hbox` does not: in vertical
/// mode its box joins the vertical list (§1076 `box_end`).
const STARTS_PAR: &[&str] = &[
    "indent",
    "noindent",
    "char",
    "unhbox",
    "unhcopy",
    "vrule",
    "accent",
    "discretionary",
    "-",
    "hskip",
    "hfil",
    "hfill",
    "hss",
    "hfilneg",
    "valign",
    " ",
    "noboundary",
    "tex_indent:D",
    "tex_noindent:D",
];

/// Commands that end a paragraph in horizontal mode: §1094
/// (`hmode+par_end`, and `hmode+stop`, `vskip`, `hrule`, `un_vbox`,
/// `halign` via §1095 `head_for_vmode`). `\penalty` and `\kern` do not.
/// e-TeX's `\pagediscards` and `\splitdiscards` are `un_vbox`.
const ENDS_PAR: &[&str] = &[
    "par",
    "end",
    "dump",
    "vskip",
    "vfil",
    "vfill",
    "vss",
    "vfilneg",
    "hrule",
    "unvbox",
    "unvcopy",
    "pagediscards",
    "splitdiscards",
    "halign",
    "tex_par:D",
];

/// An item's paragraph effect by name (`\end` with a brace after it is
/// LaTeX's, not the primitive).
pub(crate) fn implicit_of(name: &str, latex_end: bool) -> Implicit {
    if name == "end" && latex_end {
        Implicit::None
    } else if STARTS_PAR.contains(&name) {
        Implicit::StartsParagraph
    } else if ENDS_PAR.contains(&name) {
        Implicit::EndsParagraph
    } else {
        Implicit::None
    }
}

/// A character's paragraph effect (plain catcodes): letters and others
/// §1090, `$` §1090 (`math_shift`), `^` and `_` §1046
/// (`insert_dollar_sign`, then the inserted `$`).
pub(crate) fn implicit_of_char(c: char) -> Implicit {
    match c {
        '~' | '&' | '#' | '%' => Implicit::None,
        _ => Implicit::StartsParagraph,
    }
}

// ---------------------------------------------------------------------
// JSON.

fn esc(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

impl MeaningShape {
    fn json(&self, out: &mut String) {
        match self {
            MeaningShape::Primitive => out.push_str("\"Primitive\""),
            MeaningShape::Kernel => out.push_str("\"Kernel\""),
            MeaningShape::Conditional => out.push_str("\"Conditional\""),
            MeaningShape::Macro { params } => {
                out.push_str("{\"Macro\":{\"params\":");
                esc(out, params);
                out.push_str("}}");
            }
            MeaningShape::Alias(t) => {
                out.push_str("{\"Alias\":");
                esc(out, t);
                out.push('}');
            }
            MeaningShape::Register(t) => {
                out.push_str("{\"Register\":");
                esc(out, &t.to_string());
                out.push('}');
            }
            MeaningShape::Expl3(s) => {
                out.push_str("{\"Expl3\":");
                esc(out, s);
                out.push('}');
            }
        }
    }
}

impl Assumption {
    fn json(&self, out: &mut String) {
        match self {
            Assumption::MeaningIs { cs: c, expected } => {
                out.push_str("{\"MeaningIs\":{\"cs\":");
                esc(out, &cs(c));
                out.push_str(",\"expected\":");
                expected.json(out);
                out.push_str("}}");
            }
            Assumption::CatcodeIs { byte, cat } => {
                let _ = write!(out, "{{\"CatcodeIs\":{{\"byte\":{byte},\"cat\":{cat}}}}}");
            }
        }
    }
}

impl ScopeKind {
    fn json(&self, out: &mut String) {
        match self {
            ScopeKind::Simple => out.push_str("\"Simple\""),
            ScopeKind::SemiSimple => out.push_str("\"SemiSimple\""),
            ScopeKind::Math => out.push_str("\"M\""),
            ScopeKind::Display => out.push_str("\"D\""),
            ScopeKind::HBox => out.push_str("\"H\""),
            ScopeKind::VBox => out.push_str("\"V\""),
            ScopeKind::Env(e) => {
                out.push_str("{\"Env\":");
                esc(out, e);
                out.push('}');
            }
        }
    }
}

fn defs_json(out: &mut String, v: &[Def]) {
    out.push('[');
    for (k, d) in v.iter().enumerate() {
        if k > 0 {
            out.push(',');
        }
        out.push_str("{\"field\":");
        esc(out, &d.field);
        let _ = write!(out, ",\"global\":{}}}", d.global);
    }
    out.push(']');
}

impl Hints {
    /// The hints as one JSON object (enums externally tagged, as serde
    /// would write them).
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut o = String::with_capacity(128);
        self.json(&mut o);
        o
    }

    pub(crate) fn json(&self, o: &mut String) {
        o.push_str("{\"claim\":");
        match &self.claim {
            Claim::Known(n) => {
                let _ = write!(o, "{{\"Known\":{n}}}");
            }
            Claim::Predicted {
                n_tokens,
                confirm,
                also,
            } => {
                let _ = write!(o, "{{\"Predicted\":{{\"n_tokens\":{n_tokens},\"confirm\":");
                confirm.json(o);
                o.push_str(",\"also\":[");
                for (k, a) in also.iter().enumerate() {
                    if k > 0 {
                        o.push(',');
                    }
                    a.json(o);
                }
                o.push_str("]}}");
            }
            Claim::Pending { cause } => {
                o.push_str("{\"Pending\":{\"cause\":");
                esc(o, cause);
                o.push_str("}}");
            }
        }
        o.push_str(",\"may_define\":");
        match &self.may_define {
            MayDefine::Nothing => o.push_str("\"Nothing\""),
            MayDefine::Any => o.push_str("\"Any\""),
            MayDefine::Names(v) => {
                o.push_str("{\"Names\":");
                defs_json(o, v);
                o.push('}');
            }
            MayDefine::Prefixes(v) => {
                o.push_str("{\"Prefixes\":");
                defs_json(o, v);
                o.push('}');
            }
        }
        o.push_str(",\"scope\":");
        let effect = |o: &mut String, e: &Effect| match e {
            Effect::Open(k) => {
                o.push_str("{\"Open\":");
                k.json(o);
                o.push('}');
            }
            Effect::Close => o.push_str("\"Close\""),
        };
        match &self.scope {
            ScopeHint::None => o.push_str("\"None\""),
            ScopeHint::Open(k) => effect(o, &Effect::Open(k.clone())),
            ScopeHint::Close => o.push_str("\"Close\""),
            ScopeHint::Predicted { effect: e, confirm } => {
                o.push_str("{\"Predicted\":{\"effect\":");
                effect(o, e);
                o.push_str(",\"confirm\":");
                confirm.json(o);
                o.push_str("}}");
            }
            ScopeHint::Pending { cause } => {
                o.push_str("{\"Pending\":{\"cause\":");
                esc(o, cause);
                o.push_str("}}");
            }
        }
        o.push_str(",\"implicit\":");
        o.push_str(match self.implicit {
            Implicit::None => "\"None\"",
            Implicit::StartsParagraph => "\"StartsParagraph\"",
            Implicit::EndsParagraph => "\"EndsParagraph\"",
        });
        o.push_str(",\"reads\":[");
        for (k, r) in self.reads.iter().enumerate() {
            if k > 0 {
                o.push(',');
            }
            esc(o, r);
        }
        o.push_str("],\"cause\":");
        match self.cause {
            Some(c) => esc(o, c),
            None => o.push_str("null"),
        }
        o.push('}');
    }
}

/// One line of [`super::Pass::hints_jsonl`]: where, what, and the hints.
pub(crate) fn line_json(
    o: &mut String,
    ix: usize,
    atom: &Atom,
    text: &str,
    name: Option<&str>,
    h: &Hints,
) {
    let _ = write!(
        o,
        "{{\"atom\":{ix},\"start\":{},\"end\":{},\"name\":",
        atom.start, atom.end
    );
    match name {
        Some(n) => esc(o, n),
        None => esc(o, &text[atom.start..atom.end]),
    }
    o.push_str(",\"hints\":");
    h.json(o);
    o.push_str("}\n");
}

// ---------------------------------------------------------------------
// Marks.

fn brace_scope(tz: super::Status, effect: Effect, byte: u8, cat: u8) -> ScopeHint {
    match tz {
        super::Status::Known => match effect {
            Effect::Open(k) => ScopeHint::Open(k),
            Effect::Close => ScopeHint::Close,
        },
        super::Status::Predicted => ScopeHint::Predicted {
            effect,
            confirm: Assumption::CatcodeIs { byte, cat },
        },
        super::Status::Pending => ScopeHint::Pending {
            cause: "tokenization pending (catcode)",
        },
    }
}

/// The marks of a text: braces (but a box's own `{`, which its item
/// claims), math shifts (`$$` one mark claiming the second `$`), blank
/// lines, and the first character of each run of characters; none inside
/// a definition's body, which does not run where it stands.
#[allow(clippy::too_many_lines)]
pub(crate) fn marks(
    _text: &str,
    atoms: &[Atom],
    tokens: &[super::Token],
    items: &[super::Item],
) -> Vec<Mark> {
    // (atoms an item claims, but not inside a group it claims whole:
    // a box's `{`, an argument's braces, a keyword's letters)
    let mut claimed = vec![false; atoms.len()];
    for it in items {
        let n = match it.hints.claim {
            Claim::Known(n) | Claim::Predicted { n_tokens: n, .. } => n,
            Claim::Pending { .. } => it.end.saturating_sub(it.atom + 1),
        };
        let mut p = it.atom + 1;
        let end = (it.atom + 1 + n).min(atoms.len());
        while p < end {
            claimed[p] = true;
            if atoms[p].kind == AtomKind::Open && atoms[p].mate != usize::MAX && atoms[p].mate < end
            {
                p = atoms[p].mate;
                claimed[p] = true;
            }
            p += 1;
        }
    }
    let mut out = Vec::new();
    // (per brace level: 0 not in math, 1 in `$`, 2 in `$$`)
    let mut math: Vec<u8> = vec![0];
    let mut in_run = false;
    let mut k = 0;
    while k < atoms.len() {
        let at = atoms[k];
        // (plain text: a space, or a character other than `$` inside a
        // run, makes no mark and changes nothing; its token is not read)
        match at.kind {
            AtomKind::Space => {
                k += 1;
                continue;
            }
            AtomKind::Char(c) if in_run && c != '$' => {
                k += 1;
                continue;
            }
            _ => {}
        }
        let tok = tokens[at.tok];
        let mut emit = !tok.in_body && !claimed[k];
        let here = k;
        // (most atoms are characters inside a run, spaces or commands:
        // no mark, nothing made for them)
        match at.kind {
            AtomKind::Char(c) if c != '$' || tok.in_body => {
                let implicit = implicit_of_char(c);
                emit = emit && !in_run && implicit != Implicit::None;
                in_run = true;
                if emit {
                    let mut h = Hints::none();
                    h.implicit = implicit;
                    // (what a mark reads: made only for a mark kept)
                    h.reads.push(format!("catcode:{}", u32::from(c)));
                    out.push(Mark {
                        atom: here,
                        hints: h,
                    });
                }
                k += 1;
                continue;
            }
            AtomKind::Space => {
                k += 1;
                continue;
            }
            AtomKind::Cs | AtomKind::Param(_) | AtomKind::Verb => {
                in_run = false;
                k += 1;
                continue;
            }
            _ => {}
        }
        let mut h = Hints::none();
        match at.kind {
            AtomKind::Open => {
                math.push(0);
                in_run = false;
                h.scope = brace_scope(tok.tz, Effect::Open(ScopeKind::Simple), b'{', 1);
                h.reads.push("catcode:123".to_owned());
            }
            AtomKind::Close => {
                if math.len() > 1 {
                    math.pop();
                }
                in_run = false;
                h.scope = brace_scope(tok.tz, Effect::Close, b'}', 2);
                h.reads.push("catcode:125".to_owned());
            }
            AtomKind::Par => {
                // (a blank line is `\par`, §347; §1094)
                in_run = false;
                h.implicit = Implicit::EndsParagraph;
                h.reads.push(cs("par"));
            }
            AtomKind::Char('$') if !tok.in_body => {
                in_run = false;
                let display = atoms.get(k + 1).map(|x| x.kind) == Some(AtomKind::Char('$'));
                let st = math.last_mut().expect("a level");
                match *st {
                    0 => {
                        h.implicit = Implicit::StartsParagraph;
                        if display {
                            *st = 2;
                            h.claim = Claim::Known(1);
                            h.scope = ScopeHint::Open(ScopeKind::Display);
                        } else {
                            *st = 1;
                            h.scope = ScopeHint::Open(ScopeKind::Math);
                        }
                    }
                    1 => {
                        *st = 0;
                        h.scope = ScopeHint::Close;
                    }
                    _ => {
                        *st = 0;
                        h.scope = ScopeHint::Close;
                        if display {
                            h.claim = Claim::Known(1);
                        }
                    }
                }
                if tok.tz != super::Status::Known {
                    h.scope = brace_scope(
                        tok.tz,
                        match h.scope {
                            ScopeHint::Close => Effect::Close,
                            _ if display => Effect::Open(ScopeKind::Display),
                            _ => Effect::Open(ScopeKind::Math),
                        },
                        b'$',
                        3,
                    );
                }
                h.reads.push("catcode:36".to_owned());
                if display {
                    k += 1;
                }
            }
            // (taken above)
            AtomKind::Char(_)
            | AtomKind::Space
            | AtomKind::Cs
            | AtomKind::Param(_)
            | AtomKind::Verb => emit = false,
        }
        if emit {
            out.push(Mark {
                atom: here,
                hints: h,
            });
        }
        k += 1;
    }
    out
}
