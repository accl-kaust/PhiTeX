//! What the layer knows of LaTeX: the commands it reads, each with the
//! meaning it assumes (LaTeX's, or a standard package's), and the packages
//! known to change one of those meanings.
//!
//! Every name here is guarded: the facts read through it hold only if it
//! means what the table says. A document that defines one of them breaks
//! that guard (`views.rs`), and so does a package in [`package_breaks`].

use std::sync::OnceLock;

use crate::FxMap;

/// How the layer reads a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Construct {
    /// `\section*[short]{title}`, at a level: `\part` −1, `\chapter` 0,
    /// `\section` 1, … `\subparagraph` 5.
    Heading(i8),
    /// `\addcontentsline{toc}{section}{title}`.
    AddContentsLine,
    /// `\label[type]{key}`.
    Label,
    /// A reference: `list` if its argument is a comma list (cleveref's),
    /// `two` if it takes two keys (`\crefrange{a}{b}`), `opt_key` if the
    /// key is the optional argument (`\hyperref[key]{text}`).
    Ref {
        list: bool,
        two: bool,
        opt_key: bool,
    },
    /// A citation: `multi` for biblatex's `\cites{a}{b}…`, `nocite` for
    /// `\nocite`.
    Cite {
        multi: bool,
        nocite: bool,
    },
    /// A file read as TeX.
    Input(InputKind),
    /// `\InputIfFileExists{file}{then}{else}`.
    InputIfFileExists,
    /// `\IfFileExists{file}{then}{else}`.
    IfFileExists,
    /// `\includeonly{a,b}`.
    IncludeOnly,
    /// `\bibliography{a,b}` (`.bib` added).
    Bibliography,
    /// `\addbibresource[options]{file.bib}` and its kin (as named).
    AddBibResource,
    /// A file read as data.
    Asset(AssetKind),
    /// `\graphicspath{{a/}{b/}}`.
    GraphicsPath,
    Begin,
    End,
    /// A definition, by its syntax.
    Define(Definer),
    /// A counter changed.
    Counter(CounterOp),
    /// `\frontmatter`, `\mainmatter`, `\backmatter`, `\appendix`.
    Matter(Matter),
    DocumentClass,
    /// `\usepackage` and `\RequirePackage`.
    UsePackage,
    /// `\iffalse`: what follows up to the matching `\fi` (or `\else`) is
    /// skipped.
    IfFalse,
    /// `\endinput`: the file ends with the line.
    EndInput,
}

/// How a file is read as TeX.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InputKind {
    /// `\input{file}` or `\input file`: `.tex` tried first.
    Input,
    /// `\include{file}`: `file.tex`.
    Include,
    /// `\subfile{file}` (subfiles).
    Subfile,
    /// `\import{dir}{file}` (import).
    Import,
    /// A class or package of the document's own (`\documentclass{x}` with
    /// `x.cls` beside the document).
    Package,
}

/// How a file is read as data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AssetKind {
    /// `\includegraphics`: the graphics extensions tried.
    Graphics,
    /// `\lstinputlisting`, `\verbatiminput`, `\inputminted`, …: as named.
    Listing,
    /// `\includepdf` (pdfpages).
    Pdf,
}

/// The syntax of a definition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Definer {
    /// `\newcommand*{\name}[n][default]{body}`, and `\renewcommand`,
    /// `\DeclareRobustCommand` (`robust`), `\providecommand` (`provide`).
    NewCommand { provide: bool, robust: bool },
    /// `\NewDocumentCommand{\name}{spec}{body}` (xparse) and its kin.
    DocumentCommand,
    /// `\def\name#1#2{body}`, `\gdef`, `\edef`, `\xdef`.
    Def,
    /// `\let\name=\other`.
    Let,
    /// `\newenvironment{name}[n][default]{begin}{end}` and its kin.
    NewEnvironment,
    /// `\NewDocumentEnvironment{name}{spec}{begin}{end}` and its kin.
    DocumentEnvironment,
    /// An environment defined by a package's command whose code the layer
    /// does not read (`\newtheorem{name}…`, `\newtcolorbox{name}…`,
    /// `\NewEnviron{name}…`).
    OtherEnvironment,
    /// An environment whose body is verbatim (`\lstnewenvironment{name}`,
    /// fancyvrb's `\DefineVerbatimEnvironment{name}`): the syntax tree
    /// reads its body as TeX.
    VerbatimEnvironment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CounterOp {
    Set,
    Add,
    /// `\stepcounter`, `\refstepcounter`: one more, and the counters it
    /// resets back to 0.
    Step,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Matter {
    Front,
    Main,
    Back,
    Appendix,
}

/// The sectioning commands, by level from −1.
pub const LEVELS: [&str; 7] = [
    "part",
    "chapter",
    "section",
    "subsection",
    "subsubsection",
    "paragraph",
    "subparagraph",
];

const fn r(list: bool, two: bool, opt_key: bool) -> Construct {
    Construct::Ref { list, two, opt_key }
}

const fn cite(multi: bool, nocite: bool) -> Construct {
    Construct::Cite { multi, nocite }
}

const fn newcommand(provide: bool, robust: bool) -> Construct {
    Construct::Define(Definer::NewCommand { provide, robust })
}

/// The commands the layer reads, by meaning.
static TABLE: &[(&[&str], Construct)] = &[
    (&["part"], Construct::Heading(-1)),
    (&["chapter"], Construct::Heading(0)),
    (&["section"], Construct::Heading(1)),
    (&["subsection"], Construct::Heading(2)),
    (&["subsubsection"], Construct::Heading(3)),
    (&["paragraph"], Construct::Heading(4)),
    (&["subparagraph"], Construct::Heading(5)),
    (&["addcontentsline"], Construct::AddContentsLine),
    (&["label"], Construct::Label),
    (
        &[
            "ref",
            "pageref",
            "eqref",
            "autoref",
            "Autoref",
            "autopageref",
            "nameref",
            "Nameref",
            "vref",
            "Vref",
            "vpageref",
            "Vpageref",
            "fullref",
            "subref",
            "titleref",
        ],
        r(false, false, false),
    ),
    (
        &[
            "cref",
            "Cref",
            "cpageref",
            "Cpageref",
            "labelcref",
            "labelcpageref",
            "namecref",
            "nameCref",
            "lcnamecref",
            "namecrefs",
            "nameCrefs",
            "lcnamecrefs",
        ],
        r(true, false, false),
    ),
    (
        &[
            "crefrange",
            "Crefrange",
            "cpagerefrange",
            "Cpagerefrange",
            "vrefrange",
            "Vrefrange",
        ],
        r(false, true, false),
    ),
    (&["hyperref"], r(false, false, true)),
    (
        &[
            "cite",
            "Cite",
            "citep",
            "citet",
            "citealt",
            "citealp",
            "Citep",
            "Citet",
            "Citealt",
            "Citealp",
            "citeauthor",
            "Citeauthor",
            "citeyear",
            "citeyearpar",
            "citenum",
            "parencite",
            "Parencite",
            "textcite",
            "Textcite",
            "autocite",
            "Autocite",
            "footcite",
            "footcitetext",
            "smartcite",
            "Smartcite",
            "supercite",
            "fullcite",
            "footfullcite",
            "citetitle",
            "citeurl",
            "citedate",
        ],
        cite(false, false),
    ),
    (
        &[
            "cites",
            "Cites",
            "parencites",
            "Parencites",
            "textcites",
            "Textcites",
            "autocites",
            "Autocites",
            "footcites",
            "smartcites",
            "Smartcites",
            "supercites",
        ],
        cite(true, false),
    ),
    (&["nocite"], cite(false, true)),
    (&["input"], Construct::Input(InputKind::Input)),
    (&["include"], Construct::Input(InputKind::Include)),
    (&["subfile"], Construct::Input(InputKind::Subfile)),
    (
        &[
            "import",
            "subimport",
            "inputfrom",
            "subinputfrom",
            "includefrom",
            "subincludefrom",
        ],
        Construct::Input(InputKind::Import),
    ),
    (&["InputIfFileExists"], Construct::InputIfFileExists),
    (&["IfFileExists"], Construct::IfFileExists),
    (&["includeonly"], Construct::IncludeOnly),
    (&["bibliography"], Construct::Bibliography),
    (
        &["addbibresource", "addglobalbib", "addsectionbib"],
        Construct::AddBibResource,
    ),
    (
        &["includegraphics", "includesvg"],
        Construct::Asset(AssetKind::Graphics),
    ),
    (
        &[
            "lstinputlisting",
            "verbatiminput",
            "VerbatimInput",
            "inputminted",
        ],
        Construct::Asset(AssetKind::Listing),
    ),
    (&["includepdf"], Construct::Asset(AssetKind::Pdf)),
    (&["graphicspath"], Construct::GraphicsPath),
    (&["begin"], Construct::Begin),
    (&["end"], Construct::End),
    (&["newcommand", "renewcommand"], newcommand(false, false)),
    (&["providecommand"], newcommand(true, false)),
    (&["DeclareRobustCommand"], newcommand(false, true)),
    (
        &[
            "NewDocumentCommand",
            "RenewDocumentCommand",
            "ProvideDocumentCommand",
            "DeclareDocumentCommand",
            "NewExpandableDocumentCommand",
            "RenewExpandableDocumentCommand",
            "DeclareExpandableDocumentCommand",
        ],
        Construct::Define(Definer::DocumentCommand),
    ),
    (
        &["def", "gdef", "edef", "xdef"],
        Construct::Define(Definer::Def),
    ),
    (&["let"], Construct::Define(Definer::Let)),
    (
        &["newenvironment", "renewenvironment", "provideenvironment"],
        Construct::Define(Definer::NewEnvironment),
    ),
    (
        &[
            "NewDocumentEnvironment",
            "RenewDocumentEnvironment",
            "ProvideDocumentEnvironment",
            "DeclareDocumentEnvironment",
        ],
        Construct::Define(Definer::DocumentEnvironment),
    ),
    (
        &[
            "newtheorem",
            "newtcolorbox",
            "renewtcolorbox",
            "DeclareTColorBox",
            "NewTColorBox",
            "NewEnviron",
            "RenewEnviron",
            "newmdenv",
        ],
        Construct::Define(Definer::OtherEnvironment),
    ),
    (
        &[
            "lstnewenvironment",
            "DefineVerbatimEnvironment",
            "newminted",
        ],
        Construct::Define(Definer::VerbatimEnvironment),
    ),
    (&["setcounter"], Construct::Counter(CounterOp::Set)),
    (&["addtocounter"], Construct::Counter(CounterOp::Add)),
    (
        &["stepcounter", "refstepcounter"],
        Construct::Counter(CounterOp::Step),
    ),
    (&["frontmatter"], Construct::Matter(Matter::Front)),
    (&["mainmatter"], Construct::Matter(Matter::Main)),
    (&["backmatter"], Construct::Matter(Matter::Back)),
    (&["appendix"], Construct::Matter(Matter::Appendix)),
    (&["documentclass"], Construct::DocumentClass),
    (&["usepackage", "RequirePackage"], Construct::UsePackage),
    (&["iffalse"], Construct::IfFalse),
    (&["endinput"], Construct::EndInput),
];

/// What the layer reads `\name` as, if it reads it: its name as the
/// table has it, and how.
pub(crate) fn construct(name: &str) -> Option<(&'static str, Construct)> {
    static MAP: OnceLock<FxMap<&'static str, Construct>> = OnceLock::new();
    MAP.get_or_init(|| {
        TABLE
            .iter()
            .flat_map(|(names, c)| names.iter().map(move |n| (*n, *c)))
            .collect()
    })
    .get_key_value(name)
    .map(|(k, v)| (*k, *v))
}

/// LaTeX's internal commands that implement a construct: a document that
/// redefines one (between `\makeatletter` and `\makeatother`) breaks the
/// construct's guard as if it redefined the command itself.
pub(crate) fn internal(name: &str) -> Option<&'static str> {
    Some(match name {
        "@sect" | "@ssect" | "@startsection" => "section",
        "@chapter" | "@schapter" => "chapter",
        "@part" | "@spart" => "part",
        "@setref" => "ref",
        "@newl@bel" => "label",
        "@citex" => "cite",
        "@iinput" | "@input" => "input",
        "@include" => "include",
        _ => return None,
    })
}

/// A conditional TeX counts while it skips (the primitives, `\newif`'s
/// and LaTeX's own `\if@…`), as far as a name tells: not etoolbox's and
/// ifthen's commands, which take arguments and need no `\fi`, nor `\iff`.
pub(crate) fn is_conditional(name: &str) -> bool {
    name.starts_with("if")
        && !matches!(
            name,
            "iff"
                | "ifthenelse"
                | "iftoggle"
                | "ifbool"
                | "ifdef"
                | "ifundef"
                | "ifcsdef"
                | "ifcsundef"
                | "ifdefmacro"
                | "ifdefempty"
                | "ifcsempty"
                | "ifdefvoid"
                | "ifcsvoid"
                | "ifstrequal"
                | "ifstrempty"
                | "ifblank"
                | "ifnumcomp"
                | "ifnumequal"
                | "ifnumgreater"
                | "ifnumless"
                | "ifnumodd"
                | "ifdimcomp"
                | "ifdimequal"
                | "ifboolexpr"
                | "ifboolexpe"
                | "ifdefstring"
                | "ifcsstring"
                | "ifdefstrequal"
                | "ifdefltxprotect"
                | "ifdefprotected"
                | "ifcsprotected"
                | "ifdefcounter"
                | "ifcscounter"
                | "ifltxcounter"
                | "ifdeflength"
                | "ifcslength"
                | "ifdefdimen"
                | "ifcsdimen"
                | "ifrmnum"
                | "ifinlist"
                | "ifinlistcs"
                | "ifdefparam"
                | "ifcsparam"
        )
}

/// What loading a package does to the layer's guards: the commands whose
/// facts it makes unreliable, and why. Kept small: packages that redefine
/// a construct compatibly (hyperref, cleveref, titlesec, natbib,
/// biblatex, …) are not here.
pub(crate) fn package_breaks(package: &str) -> &'static [&'static str] {
    match package {
        // (\externaldocument: a reference may name another document's label)
        "xr" | "xr-hyper" | "zref-xr" => &[
            "ref", "pageref", "eqref", "autoref", "nameref", "vref", "cref", "Cref", "cpageref",
        ],
        // (an \input in an \import-ed file is relative to that file)
        "import" => &["input", "include"],
        _ => &[],
    }
}

/// Why [`package_breaks`] lists a package.
pub(crate) fn package_cause(package: &str) -> &'static str {
    match package {
        "xr" | "xr-hyper" | "zref-xr" => {
            "references may name labels of another document (\\externaldocument)"
        }
        "import" => "an \\input in an \\import-ed file is relative to that file",
        _ => "",
    }
}
