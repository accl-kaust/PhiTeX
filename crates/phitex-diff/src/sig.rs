//! The signature table: which commands take which arguments, and which of
//! them may sit inside `\DIFadd{…}`/`\DIFdel{…}` — data, after latexdiff's
//! own lists (`SAFECMD`, `TEXTCMD`, `CONTEXT1CMD`, `CONTEXT2CMD`,
//! `MATHENV`, `MATHARRENV`, `FLOATENV`, `PICTUREENV`, `LISTENV`,
//! `COUNTERCMD`, the cite family it wraps in `\mbox`), extended from the
//! document's preamble (`\newcommand` and friends: arity, and whether the
//! body is safe) and by the caller ([`Signatures::define`]).
//!
//! The CST reads with plain catcodes and knows no command's arguments: a
//! command's arguments are what its signature says follows it. A command
//! the table does not know takes the brace and bracket groups right after
//! it (no space between) and is unsafe.

use std::collections::{HashMap, HashSet};

/// What a command is to the diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Can sit, with its arguments, inside `\DIFadd{…}` (latexdiff's
    /// `SAFECMD`).
    Safe,
    /// Its last mandatory argument is text, diffed inside it (latexdiff's
    /// `TEXTCMD`). `counter`: deleting it steps that counter back (latexdiff's
    /// `COUNTERCMD`).
    Text { counter: bool },
    /// A text command that fails out of context: when deleted, only its text
    /// is shown (deleted) (`\caption`; latexdiff's `CONTEXT1CMD`).
    Context1,
    /// A text command that fails out of context: when deleted, it is
    /// commented out whole (`\title`; latexdiff's `CONTEXT2CMD`).
    Context2,
    /// Safe once wrapped in `\mbox{…}` (the cite family under ulem).
    Mbox,
    /// Anything else: an added one stays outside the markup, a deleted one
    /// is commented out.
    Unsafe,
}

/// A command's signature: its arguments, as a string of `s` (an optional
/// star), `o` (an optional `[…]` argument) and `m` (a mandatory one), and
/// its class.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sig {
    pub spec: String,
    pub class: Class,
}

impl Sig {
    #[must_use]
    pub fn new(spec: &str, class: Class) -> Sig {
        Sig {
            spec: spec.to_owned(),
            class,
        }
    }
}

/// What an environment is to the diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnvKind {
    /// Display math (latexdiff's `MATHENV`): a unit, shown whole.
    Math,
    /// Display math with alignment (latexdiff's `MATHARRENV`).
    MathArr,
    /// Inline math as an environment (`math`).
    InlineMath,
    /// A float (latexdiff's `FLOATENV`): the `FL` markup inside.
    Float,
    /// No markup inside (latexdiff's `PICTUREENV`): a unit.
    Picture,
    /// A list (latexdiff's `LISTENV`).
    List,
    /// An alignment, diffed by rows.
    Tabular,
    /// Any other: its body is diffed.
    Plain,
}

/// An environment's signature: the arguments after `\begin{name}`, and its
/// kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvSig {
    pub spec: String,
    pub kind: EnvKind,
}

/// The signature table.
#[derive(Clone, Debug, Default)]
pub struct Signatures {
    cmds: HashMap<String, Sig>,
    envs: HashMap<String, EnvSig>,
    /// Commands defined by the old preamble and not the new one: a deleted
    /// use of one would not compile, so it is commented out.
    old_only: HashSet<String>,
}

/// latexdiff's `SAFECMD` list, as names (its patterns are in
/// [`pattern_class`]): commands with no argument or with one.
const SAFE0: &[&str] = &[
    "arabic",
    "dag",
    "ddag",
    "copyright",
    "pounds",
    "i",
    "S",
    "P",
    "oe",
    "OE",
    "ae",
    "AE",
    "aa",
    "AA",
    "o",
    "O",
    "l",
    "L",
    "ss",
    "ldots",
    "cdots",
    "vdots",
    "ddots",
    "dots",
    "alpha",
    "beta",
    "gamma",
    "delta",
    "epsilon",
    "varepsilon",
    "zeta",
    "eta",
    "theta",
    "vartheta",
    "iota",
    "kappa",
    "lambda",
    "mu",
    "nu",
    "xi",
    "pi",
    "varpi",
    "rho",
    "varrho",
    "sigma",
    "varsigma",
    "tau",
    "upsilon",
    "phi",
    "varphi",
    "chi",
    "psi",
    "omega",
    "Gamma",
    "Delta",
    "Theta",
    "Lambda",
    "Xi",
    "Pi",
    "Sigma",
    "Upsilon",
    "Phi",
    "Psi",
    "Omega",
    "ps",
    "mp",
    "pm",
    "times",
    "div",
    "ast",
    "star",
    "circ",
    "bullet",
    "cdot",
    "cap",
    "cup",
    "uplus",
    "sqcap",
    "vee",
    "wedge",
    "setminus",
    "wr",
    "diamond",
    "lhd",
    "rhd",
    "unlhd",
    "unrhd",
    "oplus",
    "ominus",
    "otimes",
    "oslash",
    "odot",
    "bigcirc",
    "dagger",
    "ddagger",
    "amalg",
    "leq",
    "le",
    "prec",
    "preceq",
    "ll",
    "in",
    "vdash",
    "geq",
    "ge",
    "succ",
    "succeq",
    "gg",
    "ni",
    "dashv",
    "equiv",
    "sim",
    "simeq",
    "asymp",
    "approx",
    "cong",
    "neq",
    "ne",
    "doteq",
    "propto",
    "models",
    "perp",
    "mid",
    "parallel",
    "bowtie",
    "Join",
    "smile",
    "frown",
    "mapsto",
    "longmapsto",
    "leadsto",
    "aleph",
    "hbar",
    "imath",
    "jmath",
    "ell",
    "wp",
    "Re",
    "Im",
    "mho",
    "prime",
    "emptyset",
    "nabla",
    "surd",
    "top",
    "bot",
    "angle",
    "forall",
    "exists",
    "neg",
    "flat",
    "natural",
    "sharp",
    "backslash",
    "partial",
    "infty",
    "Box",
    "Diamond",
    "triangle",
    "clubsuit",
    "diamondsuit",
    "heartsuit",
    "spadesuit",
    "sum",
    "prod",
    "coprod",
    "int",
    "oint",
    "bigcap",
    "bigcup",
    "bigsqcup",
    "bigvee",
    "bigwedge",
    "bigodot",
    "bigotimes",
    "bigoplus",
    "biguplus",
    "csc",
    "arg",
    "deg",
    "det",
    "dim",
    "exp",
    "gcd",
    "hom",
    "inf",
    "ker",
    "lg",
    "lim",
    "liminf",
    "limsup",
    "ln",
    "log",
    "max",
    "min",
    "Pr",
    "sec",
    "sup",
    "quad",
    "qquad",
    "ensuremath",
    // (control symbols: latexdiff's PERCENTAGE, DOLLAR, AMPERSAND, _,
    // QLEFTBRACE, QRIGHTBRACE, and the spacing ones)
    "%",
    "$",
    "&",
    "_",
    "#",
    "{",
    "}",
    ",",
    ";",
    ":",
    "!",
    " ",
    "@",
    "/",
    "-",
];

/// Safe commands with arguments.
const SAFE_ARGS: &[(&str, &str)] = &[
    ("emph", "m"),
    ("fbox", "m"),
    ("mbox", "m"),
    ("hspace", "sm"),
    ("pageref", "sm"),
    ("ref", "sm"),
    ("eqref", "m"),
    ("autoref", "sm"),
    ("nameref", "sm"),
    ("symbol", "m"),
    ("rule", "omm"),
    ("usebox", "m"),
    ("frac", "mm"),
    ("dfrac", "mm"),
    ("tfrac", "mm"),
    ("binom", "mm"),
    ("bibfield", "mm"),
    ("bibinfo", "mm"),
    ("ensuremath", "m"),
    ("overline", "m"),
    ("underbrace", "m"),
    ("overbrace", "m"),
    ("hat", "m"),
    ("widehat", "m"),
    ("tilde", "m"),
    ("widetilde", "m"),
    ("bar", "m"),
    ("vec", "m"),
    ("dot", "m"),
    ("ddot", "m"),
    ("acute", "m"),
    ("grave", "m"),
    ("breve", "m"),
    ("check", "m"),
    ("operatorname", "sm"),
];

/// Text commands (latexdiff's `TEXTCMD`; the last mandatory argument is
/// the text), with whether they count (`COUNTERCMD`).
const TEXT: &[(&str, &str, bool)] = &[
    ("part", "som", true),
    ("chapter", "som", true),
    ("section", "som", true),
    ("subsection", "som", true),
    ("subsubsection", "som", true),
    ("paragraph", "som", true),
    ("subparagraph", "som", true),
    ("footnote", "om", true),
    ("footnotetext", "om", false),
    ("addcontentsline", "mmm", false),
    ("addtocontents", "mm", false),
    ("cc", "m", false),
    ("closing", "m", false),
    ("encl", "m", false),
    ("opening", "m", false),
    ("signature", "m", false),
    ("dashbox", "moom", false),
    ("framebox", "oom", false),
    ("makebox", "oom", false),
    ("emph", "m", false),
    ("fbox", "m", false),
    ("mbox", "m", false),
    ("href", "mm", false),
    ("intertext", "m", false),
    ("shortintertext", "m", false),
    ("parbox", "ooomm", false),
    ("raisebox", "moom", false),
    ("savebox", "moom", false),
    ("sbox", "mm", false),
    ("shortstack", "om", false),
    ("sidenote", "om", false),
    ("value", "m", false),
    ("underline", "m", false),
    ("sqrt", "om", false),
    ("text", "m", false),
    ("textcolor", "omm", false),
    ("colorbox", "omm", false),
    ("textbf", "m", false),
    ("textit", "m", false),
    ("texttt", "m", false),
    ("textsc", "m", false),
    ("textsf", "m", false),
    ("textrm", "m", false),
    ("textup", "m", false),
    ("textmd", "m", false),
    ("textsl", "m", false),
    ("textnormal", "m", false),
    ("textsuperscript", "m", false),
    ("textsubscript", "m", false),
    ("textcircled", "m", false),
    ("uline", "m", false),
];

/// Latexdiff's `CONTEXT1CMD` and `CONTEXT2CMD`.
const CONTEXT: &[(&str, &str, Class)] = &[
    ("caption", "som", Class::Context1),
    ("subcaption", "som", Class::Context1),
    ("multicolumn", "mmm", Class::Context1),
    ("title", "om", Class::Context2),
    ("author", "om", Class::Context2),
    ("date", "m", Class::Context2),
    ("institute", "om", Class::Context2),
];

/// The cite family and cleveref's references, safe inside `\mbox`
/// (latexdiff's `MBOXCMD` under ulem).
const MBOX: &[(&str, &str)] = &[
    ("cite", "som"),
    ("citep", "soom"),
    ("citet", "soom"),
    ("citealp", "soom"),
    ("citealt", "soom"),
    ("citeauthor", "som"),
    ("citeyear", "som"),
    ("citeyearpar", "som"),
    ("Cite", "soom"),
    ("parencite", "soom"),
    ("Parencite", "soom"),
    ("textcite", "soom"),
    ("Textcite", "soom"),
    ("autocite", "soom"),
    ("Autocite", "soom"),
    ("footcite", "soom"),
    ("cref", "sm"),
    ("Cref", "sm"),
    ("crefrange", "smm"),
    ("Crefrange", "smm"),
    ("labelcref", "m"),
    ("namecref", "m"),
    ("nameCref", "m"),
    ("SI", "omm"),
    ("si", "om"),
    ("num", "om"),
    ("qty", "omm"),
    ("unit", "om"),
    ("ang", "om"),
];

/// Unsafe commands whose arguments the table knows (so that they are read
/// as one unit with them).
const UNSAFE_ARGS: &[(&str, &str)] = &[
    ("label", "m"),
    ("item", "o"),
    ("bibitem", "om"),
    ("includegraphics", "soom"),
    ("input", "m"),
    ("include", "m"),
    ("bibliography", "m"),
    ("bibliographystyle", "m"),
    ("printbibliography", "o"),
    ("vspace", "sm"),
    ("\\", "so"),
    ("tabularnewline", "o"),
    ("newline", ""),
    ("linebreak", "o"),
    ("pagebreak", "o"),
    ("nolinebreak", "o"),
    ("color", "om"),
    ("footnotemark", "o"),
    ("cline", "m"),
    ("cmidrule", "om"),
    ("addlinespace", "o"),
    ("setlength", "mm"),
    ("addtolength", "mm"),
    ("setcounter", "mm"),
    ("addtocounter", "mm"),
    ("stepcounter", "m"),
    ("refstepcounter", "m"),
    ("tag", "sm"),
    ("hyperref", "om"),
    ("hyperlink", "mm"),
    ("hypertarget", "mm"),
    ("thanks", "m"),
    ("centering", ""),
    ("maketitle", ""),
    ("tableofcontents", ""),
    ("newpage", ""),
    ("clearpage", ""),
    ("noindent", ""),
    ("par", ""),
    ("hline", ""),
    ("toprule", "o"),
    ("midrule", "o"),
    ("bottomrule", "o"),
    ("left", ""),
    ("right", ""),
];

/// Environments: name, arguments after `\begin{name}`, kind.
const ENVS: &[(&str, &str, EnvKind)] = &[
    ("equation", "", EnvKind::Math),
    ("equation*", "", EnvKind::Math),
    ("displaymath", "", EnvKind::Math),
    ("math", "", EnvKind::InlineMath),
    ("eqnarray", "", EnvKind::MathArr),
    ("eqnarray*", "", EnvKind::MathArr),
    ("align", "", EnvKind::MathArr),
    ("align*", "", EnvKind::MathArr),
    ("alignat", "m", EnvKind::MathArr),
    ("alignat*", "m", EnvKind::MathArr),
    ("gather", "", EnvKind::MathArr),
    ("gather*", "", EnvKind::MathArr),
    ("multline", "", EnvKind::MathArr),
    ("multline*", "", EnvKind::MathArr),
    ("flalign", "", EnvKind::MathArr),
    ("flalign*", "", EnvKind::MathArr),
    ("figure", "o", EnvKind::Float),
    ("figure*", "o", EnvKind::Float),
    ("table", "o", EnvKind::Float),
    ("table*", "o", EnvKind::Float),
    ("plate", "o", EnvKind::Float),
    ("plate*", "o", EnvKind::Float),
    ("picture", "", EnvKind::Picture),
    ("tikzpicture", "o", EnvKind::Picture),
    ("pgfpicture", "", EnvKind::Picture),
    ("DIFnomarkup", "", EnvKind::Picture),
    ("itemize", "o", EnvKind::List),
    ("enumerate", "o", EnvKind::List),
    ("description", "o", EnvKind::List),
    ("tabular", "om", EnvKind::Tabular),
    ("tabular*", "mom", EnvKind::Tabular),
    ("tabularx", "mom", EnvKind::Tabular),
    ("tabulary", "mom", EnvKind::Tabular),
    ("longtable", "om", EnvKind::Tabular),
    ("array", "om", EnvKind::Tabular),
    ("minipage", "ooom", EnvKind::Plain),
    ("multicols", "mo", EnvKind::Plain),
    ("thebibliography", "m", EnvKind::List),
    ("subfigure", "om", EnvKind::Plain),
    ("wrapfigure", "omom", EnvKind::Plain),
];

/// latexdiff's `SAFECMD` patterns (`math.*`, `.*arrow`, …) and the
/// `text.*` symbols: the class and arguments of a name no list has.
fn pattern_class(name: &str) -> Option<Sig> {
    let safe = |spec: &str| Some(Sig::new(spec, Class::Safe));
    if let Some(rest) = name.strip_prefix("math") {
        // (\mathbf{x}…: one argument; \mathsurround… are not commands of
        // text, but are safe)
        return match rest {
            "bf" | "rm" | "it" | "cal" | "bb" | "sf" | "tt" | "frak" | "scr" | "normal" | "bin"
            | "rel" | "op" | "ord" | "ring" | "bfit" => safe("m"),
            _ => safe(""),
        };
    }
    if name.starts_with("text") {
        // (the text.* commands with an argument are in TEXT: the rest are
        // symbols, \textendash…)
        return safe("");
    }
    if name.ends_with("arrow") || name.ends_with("arrows") || name.contains("harpoon") {
        return safe("");
    }
    if name.starts_with("triangle") || name.starts_with("bigtriangle") {
        return safe("");
    }
    if matches!(
        name,
        "subset"
            | "supset"
            | "subseteq"
            | "supseteq"
            | "sqsubset"
            | "sqsupset"
            | "sqsubseteq"
            | "sqsupseteq"
    ) {
        return safe("");
    }
    if let Some(r) = name
        .strip_prefix("arc")
        .or(Some(name))
        .filter(|r| r.len() >= 3)
    {
        let base = r.strip_suffix('h').unwrap_or(r);
        if matches!(base, "cos" | "sin" | "tan" | "cot") {
            return safe("");
        }
    }
    // (accents: \H \c \l \b \k \d \r \u \v \t and \` \' \^ \" \~ \= \.)
    if matches!(
        name,
        "H" | "c"
            | "b"
            | "k"
            | "d"
            | "r"
            | "u"
            | "v"
            | "t"
            | "`"
            | "'"
            | "^"
            | "\""
            | "~"
            | "="
            | "."
    ) {
        return safe("m");
    }
    if name.starts_with("big") || name.starts_with("Big") {
        // (\big( \Bigg| …: a delimiter follows, a token: safe as one)
        return safe("");
    }
    None
}

impl Signatures {
    /// latexdiff's defaults.
    #[must_use]
    pub fn latexdiff() -> Signatures {
        let mut s = Signatures::default();
        for n in SAFE0 {
            s.cmds.insert((*n).to_owned(), Sig::new("", Class::Safe));
        }
        for (n, spec) in SAFE_ARGS {
            s.cmds.insert((*n).to_owned(), Sig::new(spec, Class::Safe));
        }
        for (n, spec, counter) in TEXT {
            s.cmds.insert(
                (*n).to_owned(),
                Sig::new(spec, Class::Text { counter: *counter }),
            );
        }
        for (n, spec, class) in CONTEXT {
            s.cmds.insert((*n).to_owned(), Sig::new(spec, *class));
        }
        for (n, spec) in MBOX {
            s.cmds.insert((*n).to_owned(), Sig::new(spec, Class::Mbox));
        }
        for (n, spec) in UNSAFE_ARGS {
            s.cmds
                .insert((*n).to_owned(), Sig::new(spec, Class::Unsafe));
        }
        for (n, spec, kind) in ENVS {
            s.envs.insert(
                (*n).to_owned(),
                EnvSig {
                    spec: (*spec).to_owned(),
                    kind: *kind,
                },
            );
        }
        s
    }

    /// The signature of command `name` (without its backslash), if known.
    #[must_use]
    pub fn cmd(&self, name: &str) -> Option<Sig> {
        self.cmds.get(name).cloned().or_else(|| pattern_class(name))
    }

    /// The signature of environment `name`, if known.
    #[must_use]
    pub fn env(&self, name: &str) -> Option<&EnvSig> {
        self.envs.get(name)
    }

    /// Define (or redefine) command `name`.
    pub fn define(&mut self, name: &str, sig: Sig) {
        self.cmds.insert(name.to_owned(), sig);
    }

    /// Define (or redefine) environment `name`.
    pub fn define_env(&mut self, name: &str, sig: EnvSig) {
        self.envs.insert(name.to_owned(), sig);
    }

    /// Whether command `name` is defined only by the old version's preamble.
    #[must_use]
    pub fn old_only(&self, name: &str) -> bool {
        self.old_only.contains(name)
    }

    /// Mark the commands the old preamble defines and the new one does not.
    pub fn set_old_only(&mut self, old: &Learned, new: &Learned) {
        self.old_only = old
            .commands
            .iter()
            .filter(|n| !new.commands.contains(*n))
            .cloned()
            .collect();
    }

    /// Learn the definitions in `preamble`: `\newcommand`,
    /// `\renewcommand`, `\providecommand`, `\DeclareRobustCommand` (with
    /// their arity and optional argument), `\def` (its parameters),
    /// `\let`, `\DeclareMathOperator`, `\NewDocumentCommand` (m, o, O, s
    /// arguments) and `\newenvironment`. A macro is safe if every command in
    /// its body is safe. What it learnt.
    #[allow(clippy::too_many_lines, reason = "one definition command an arm")]
    pub fn learn(&mut self, preamble: &str) -> Learned {
        let toks = flat_tokens(preamble);
        let mut learned = Learned::default();
        let mut i = 0;
        while i < toks.len() {
            let (kind, text) = toks[i];
            i += 1;
            if kind != T::Cs {
                continue;
            }
            match text {
                "\\newcommand"
                | "\\renewcommand"
                | "\\providecommand"
                | "\\DeclareRobustCommand" => {
                    i = skip_star(&toks, i);
                    let Some((name, j)) = defined_name(&toks, i) else {
                        continue;
                    };
                    i = j;
                    let mut n = 0;
                    if let Some((arg, j)) = bracket(&toks, i) {
                        n = arg.trim().parse::<usize>().unwrap_or(0);
                        i = j;
                    }
                    let mut opt = false;
                    if let Some((_, j)) = bracket(&toks, i) {
                        opt = true;
                        i = j;
                    }
                    let (body, j) = group_text(&toks, i);
                    i = j;
                    let spec = if opt {
                        format!("o{}", "m".repeat(n.saturating_sub(1)))
                    } else {
                        "m".repeat(n)
                    };
                    let class = self.body_class(&body);
                    learned.commands.insert(name.clone());
                    self.define(&name, Sig { spec, class });
                }
                "\\def" | "\\gdef" | "\\edef" | "\\xdef" => {
                    let Some(&(T::Cs, name)) = toks.get(i) else {
                        continue;
                    };
                    i += 1;
                    let mut n = 0;
                    while let Some(&(k, _)) = toks.get(i) {
                        if k == T::Group {
                            break;
                        }
                        if k == T::Param {
                            n += 1;
                        }
                        i += 1;
                    }
                    let (body, j) = group_text(&toks, i);
                    i = j;
                    let class = self.body_class(&body);
                    let name = name[1..].to_owned();
                    learned.commands.insert(name.clone());
                    self.define(
                        &name,
                        Sig {
                            spec: "m".repeat(n),
                            class,
                        },
                    );
                }
                "\\let" => {
                    let Some(&(T::Cs, name)) = toks.get(i) else {
                        continue;
                    };
                    let mut j = i + 1;
                    if toks.get(j).is_some_and(|t| t.1 == "=") {
                        j += 1;
                    }
                    if let Some(&(T::Cs, target)) = toks.get(j) {
                        let sig = self
                            .cmd(&target[1..])
                            .unwrap_or_else(|| Sig::new("", Class::Unsafe));
                        learned.commands.insert(name[1..].to_owned());
                        self.define(&name[1..], sig);
                        i = j + 1;
                    }
                }
                "\\DeclareMathOperator" => {
                    i = skip_star(&toks, i);
                    if let Some((name, j)) = defined_name(&toks, i) {
                        learned.commands.insert(name.clone());
                        self.define(&name, Sig::new("", Class::Safe));
                        i = j;
                    }
                }
                "\\NewDocumentCommand"
                | "\\RenewDocumentCommand"
                | "\\ProvideDocumentCommand"
                | "\\DeclareDocumentCommand" => {
                    let Some((name, j)) = defined_name(&toks, i) else {
                        continue;
                    };
                    let (args, j) = group_text(&toks, j);
                    let (body, j) = group_text(&toks, j);
                    i = j;
                    let spec = xparse_spec(&args);
                    let class = self.body_class(&body);
                    learned.commands.insert(name.clone());
                    self.define(&name, Sig { spec, class });
                }
                "\\newenvironment" | "\\renewenvironment" => {
                    i = skip_star(&toks, i);
                    let (name, j) = group_text(&toks, i);
                    i = j;
                    let mut n = 0;
                    if let Some((arg, j)) = bracket(&toks, i) {
                        n = arg.trim().parse::<usize>().unwrap_or(0);
                        i = j;
                    }
                    let mut opt = false;
                    if let Some((_, j)) = bracket(&toks, i) {
                        opt = true;
                        i = j;
                    }
                    let spec = if opt {
                        format!("o{}", "m".repeat(n.saturating_sub(1)))
                    } else {
                        "m".repeat(n)
                    };
                    let name = name.trim().to_owned();
                    if !name.is_empty() && self.env(&name).is_none() {
                        self.define_env(
                            &name,
                            EnvSig {
                                spec,
                                kind: EnvKind::Plain,
                            },
                        );
                    }
                }
                _ => {}
            }
        }
        learned
    }

    /// A macro body's class: safe if every command in it is safe.
    fn body_class(&self, body: &str) -> Class {
        let toks = flat_tokens(body);
        let safe = toks.iter().all(|&(k, t)| {
            k != T::Cs
                || self.cmd(&t[1..]).is_some_and(|s| {
                    matches!(s.class, Class::Safe | Class::Mbox)
                        || (matches!(s.class, Class::Text { counter: false })
                            && SAFE_TEXT.contains(&&t[1..]))
                })
        });
        if safe { Class::Safe } else { Class::Unsafe }
    }
}

/// The text commands latexdiff also lists as safe.
const SAFE_TEXT: &[&str] = &[
    "emph",
    "fbox",
    "mbox",
    "text",
    "textbf",
    "textit",
    "texttt",
    "textsc",
    "textsf",
    "textrm",
    "textup",
    "textmd",
    "textsl",
    "textnormal",
    "textsuperscript",
    "textsubscript",
    "textcolor",
];

/// What a preamble defines.
#[derive(Clone, Debug, Default)]
pub struct Learned {
    pub commands: HashSet<String>,
}

/// An xparse argument specification as `s`, `o`, `m`.
fn xparse_spec(args: &str) -> String {
    let mut spec = String::new();
    let mut chars = args.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            's' => spec.push('s'),
            'o' => spec.push('o'),
            'O' => {
                spec.push('o');
                // (its default, braced)
                let mut depth = 0;
                for d in chars.by_ref() {
                    match d {
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
            'm' | 'v' => spec.push('m'),
            _ => {}
        }
    }
    spec
}

/// A preamble's tokens, flat: kind and text (a group is its whole text, as
/// one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum T {
    Cs,
    Group,
    Param,
    Other,
}

fn flat_tokens(text: &str) -> Vec<(T, &str)> {
    use phitex_syntax::{SyntaxKind as K, lex};
    let green = lex(text);
    let mut out = Vec::new();
    let mut at = 0;
    for c in green.children() {
        let s = &text[at..at + c.len()];
        at += c.len();
        match c.kind() {
            K::Cs => out.push((T::Cs, s)),
            K::Group => out.push((T::Group, s)),
            K::Param => out.push((T::Param, s)),
            K::Space | K::Newline | K::Comment => {}
            K::Text => {
                // (one entry per character: `[2]`, `*`, `=` are read by it)
                for (k, ch) in s.char_indices() {
                    out.push((T::Other, &s[k..k + ch.len_utf8()]));
                }
            }
            _ => out.push((T::Other, s)),
        }
    }
    out
}

fn skip_star(toks: &[(T, &str)], i: usize) -> usize {
    if toks.get(i).is_some_and(|t| t.1 == "*") {
        i + 1
    } else {
        i
    }
}

/// The name a definition defines: `{\name}` or `\name`.
fn defined_name(toks: &[(T, &str)], i: usize) -> Option<(String, usize)> {
    match toks.get(i)? {
        (T::Cs, n) => Some((n[1..].to_owned(), i + 1)),
        (T::Group, g) => {
            let inner = g.trim_start_matches('{').trim_end_matches('}').trim();
            let n = inner.strip_prefix('\\')?;
            Some((n.to_owned(), i + 1))
        }
        _ => None,
    }
}

/// A `[…]` at `i`: its content and what follows.
fn bracket(toks: &[(T, &str)], i: usize) -> Option<(String, usize)> {
    if toks.get(i)?.1 != "[" {
        return None;
    }
    let mut s = String::new();
    let mut j = i + 1;
    while let Some(&(_, t)) = toks.get(j) {
        j += 1;
        if t == "]" {
            return Some((s, j));
        }
        s.push_str(t);
    }
    None
}

/// The group at `i` (its content without braces) and what follows; a
/// single token if not a group.
fn group_text(toks: &[(T, &str)], i: usize) -> (String, usize) {
    match toks.get(i) {
        Some(&(T::Group, g)) => {
            let inner = g.strip_prefix('{').unwrap_or(g);
            let inner = inner.strip_suffix('}').unwrap_or(inner);
            (inner.to_owned(), i + 1)
        }
        Some(&(_, t)) => (t.to_owned(), i + 1),
        None => (String::new(), i),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let s = Signatures::latexdiff();
        assert_eq!(s.cmd("alpha").unwrap().class, Class::Safe);
        assert_eq!(s.cmd("mathbf").unwrap().spec, "m");
        assert_eq!(s.cmd("rightarrow").unwrap().class, Class::Safe);
        assert_eq!(s.cmd("arccosh").unwrap().class, Class::Safe);
        assert_eq!(
            s.cmd("section").unwrap().class,
            Class::Text { counter: true }
        );
        assert_eq!(s.cmd("caption").unwrap().class, Class::Context1);
        assert_eq!(s.cmd("cite").unwrap().class, Class::Mbox);
        assert_eq!(s.cmd("textwidth").unwrap().spec, "");
        assert!(s.cmd("unknownmacro").is_none());
        assert_eq!(s.env("align*").unwrap().kind, EnvKind::MathArr);
    }

    #[test]
    fn learns_preamble() {
        let mut s = Signatures::latexdiff();
        let l = s.learn(
            "\\newcommand{\\R}{\\mathbb{R}}\n\\newcommand\\note[2][x]{\\marginpar{#1#2}}\n\
             \\def\\pair#1#2{(#1,#2)}\n\\DeclareMathOperator{\\tr}{tr}\n\
             \\NewDocumentCommand\\foo{s o m}{\\emph{#3}}\n\\newenvironment{thm}[1][]{}{}\n",
        );
        assert_eq!(s.cmd("R").unwrap(), Sig::new("", Class::Safe));
        assert_eq!(s.cmd("note").unwrap(), Sig::new("om", Class::Unsafe));
        assert_eq!(s.cmd("pair").unwrap(), Sig::new("mm", Class::Safe));
        assert_eq!(s.cmd("tr").unwrap().class, Class::Safe);
        assert_eq!(s.cmd("foo").unwrap(), Sig::new("som", Class::Safe));
        assert_eq!(s.env("thm").unwrap().spec, "o");
        assert!(l.commands.contains("note"));
    }
}
