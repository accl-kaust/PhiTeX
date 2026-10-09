//! TeX's primitives and parameters: names and constants only.
//!
//! A primitive is what a control sequence means before any definition
//! ([`crate::val::initial`]). Nothing here holds state: the table is a
//! constant, read by [`Prim::by_name`] and [`Prim::name`].

macro_rules! params {
    ($t:ident, $n:literal, $($v:ident = $s:literal),* $(,)?) => {
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
        pub enum $t { $($v),* }
        impl $t {
            pub const ALL: [$t; $n] = [$($t::$v),*];
            pub fn name(self) -> &'static str {
                match self { $($t::$v => $s),* }
            }
        }
    };
}

/// A conditional (tex.web 487).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum IfKind {
    Char,
    Cat,
    Num,
    Dim,
    Odd,
    Vmode,
    Hmode,
    Mmode,
    Inner,
    Void,
    Hbox,
    Vbox,
    X,
    Eof,
    True,
    False,
    Case,
    /// e-TeX's `\ifdefined`, `\ifcsname`, `\iffontchar`.
    Defined,
    Csname,
    FontChar,
}

impl IfKind {
    /// Its code in tex.web/etex.web (`if_char_code` 0 … `if_font_char_code`
    /// 19): `\currentiftype` is this plus one.
    pub fn code(self) -> u8 {
        self as u8
    }
}

/// A register bank.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Reg {
    Count,
    Dimen,
    Skip,
    Muskip,
    Toks,
}

impl Reg {
    pub fn name(self) -> &'static str {
        match self {
            Reg::Count => "count",
            Reg::Dimen => "dimen",
            Reg::Skip => "skip",
            Reg::Muskip => "muskip",
            Reg::Toks => "toks",
        }
    }
}

/// `\chardef` and friends.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Shorthand {
    Char,
    MathChar,
    Count,
    Dimen,
    Skip,
    Muskip,
    Toks,
}

/// A character code table.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Code {
    Cat,
    Math,
    Lc,
    Uc,
    Sf,
    Del,
}

/// `\number` and friends (tex.web 468).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Conv {
    Number,
    Roman,
    String,
    Meaning,
    FontName,
    JobName,
    /// e-TeX's `\eTeXrevision`.
    ETeXRevision,
}

/// A primitive that makes material, boxes or pages (tex.web's commands
/// `hskip` … `leader_ship`), and the reader's own internal commands.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Mat {
    /// `\hskip` (0), `\hfil`, `\hfill`, `\hss`, `\hfilneg` (4).
    Hskip(u8),
    /// `\vskip` (0), `\vfil`, `\vfill`, `\vss`, `\vfilneg` (4).
    Vskip(u8),
    Kern,
    Penalty,
    Hrule,
    Vrule,
    /// `\unpenalty` (0), `\unkern` (1), `\unskip` (2).
    Remove(u8),
    Mark,
    /// `\box` (0), `\copy`, `\lastbox`, `\vsplit`, `\vtop`, `\vbox`,
    /// `\hbox` (6).
    MakeBox(u8),
    SetBox,
    /// `\unhbox`, `\unhcopy` (true).
    Unhbox(bool),
    /// `\unvbox`, `\unvcopy` (true).
    Unvbox(bool),
    /// `\raise` (false), `\lower` (true).
    Vmove(bool),
    /// `\moveright` (false), `\moveleft` (true).
    Hmove(bool),
    Shipout,
    /// `\-` (false), `\discretionary` (true).
    Disc(bool),
    /// `\/`.
    ItalCorr,
    /// `\hyphenchar` (0), `\skewchar` (1).
    FontInt(u8),
    /// The reader's own: a box group's contents are packaged (0); a
    /// paragraph ends before a `}` (1); a discretionary's next part
    /// begins (2); a `\vbox`'s paragraph shape is reset inside it (3).
    Internal(u8),
    /// `\leaders` (100), `\cleaders` (101), `\xleaders` (102): tex.web's
    /// glue subtypes.
    Leaders(u8),
    Accent,
    Insert,
    Vadjust,
}

/// A math command (tex.web §1136–§1206).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MathP {
    /// `\mathchar`.
    MathChar,
    /// `\delimiter`.
    Delimiter,
    /// `\mathord` (0) … `\mathinner` (7), `\underline` (8), `\overline` (9).
    Comp(u8),
    /// `\displaylimits` (0), `\limits` (1), `\nolimits` (2).
    Limits(u8),
    Radical,
    /// `\mathaccent`.
    Accent,
    Vcenter,
    /// `\displaystyle` (0), `\textstyle` (2), `\scriptstyle` (4),
    /// `\scriptscriptstyle` (6).
    Style(u8),
    NonScript,
    /// `\mathchoice`.
    Choice,
    /// `\above` (0), `\over` (1), `\atop` (2); the `withdelims` forms +3.
    Above(u8),
    Left,
    Right,
    /// `\eqno` (false), `\leqno` (true).
    EqNo(bool),
    Mskip,
    Mkern,
}

/// A primitive.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Prim {
    Def,
    Gdef,
    Edef,
    Xdef,
    Global,
    Long,
    Outer,
    Let,
    Futurelet,
    Shorthand(Shorthand),
    If(IfKind),
    Fi,
    Else,
    Or,
    Code(Code),
    Par,
    Relax,
    Indent,
    Noindent,
    /// `\ ` (control space): an interword space.
    Space,
    IntPar(IntPar),
    DimenPar(DimenPar),
    GluePar(GluePar),
    TokPar(TokPar),
    Register(Reg),
    Advance,
    Multiply,
    Divide,
    BeginGroup,
    EndGroup,
    AfterGroup,
    AfterAssignment,
    ExpandAfter,
    NoExpand,
    CsName,
    EndCsName,
    Convert(Conv),
    The,
    Input,
    EndInput,
    /// `\topmark` (0) … `\splitbotmark` (4).
    Mark(u8),
    Message,
    ErrMessage,
    Immediate,
    Write,
    Uppercase,
    Lowercase,
    Parshape,
    IgnoreSpaces,
    Char,
    /// `\font`: defines a font identifier; as an identifier, the current
    /// font.
    Font,
    FontDimen,
    SpaceFactor,
    PrevDepth,
    PrevGraf,
    /// `\deadcycles` (0), `\insertpenalties` (1).
    PageInt(u8),
    /// `\pagegoal` (0) … `\pagedepth` (7).
    PageDimen(u8),
    /// `\wd` (0), `\ht` (1), `\dp` (2).
    BoxDimen(u8),
    /// `\lastpenalty` (0), `\lastkern`, `\lastskip`, `\inputlineno`,
    /// `\badness` (4).
    LastItem(u8),
    End,
    Dump,
    /// `\batchmode` (0) … `\errorstopmode` (3).
    Interaction(u8),
    /// `\textfont` (0), `\scriptfont` (1), `\scriptscriptfont` (2).
    DefFamily(u8),
    /// Material, boxes and pages.
    Mat(Mat),
    /// `\openin` (true), `\closein` (false).
    InStream(bool),
    Read,
    OpenOut,
    CloseOut,
    Special,
    /// Math.
    Math(MathP),
    /// `\patterns` (true), `\hyphenation` (false).
    HyphData(bool),
    /// `\setlanguage`.
    SetLanguage,
    /// `\showbox` (1), `\showlists` (3) (tex.web 1291's codes).
    Show(u8),
    /// A primitive without a graph form yet: `UNIMPL[i]`.
    Unimpl(u16),
    // ---- e-TeX's (etex.web), named only in extended mode
    // (`ETEX_PRIMITIVES`); `LastItem(ETEX_ITEM..)`, `Mark(5..)` and
    // `PageInt(2)` are e-TeX's too.
    /// `\unless`: negates the boolean conditional after it.
    Unless,
    /// `\unexpanded`, `\detokenize` (`the` with chr 1 and 5).
    Unexpanded,
    Detokenize,
    /// `\scantokens`: the text becomes input, read again.
    ScanTokens,
    /// `\protected` (a prefix).
    Protected,
    /// `\readline`.
    ReadLine,
    /// `\showgroups` (4), `\showtokens` (5), `\showifs` (6).
    ShowE(u8),
    /// `\marks`.
    Marks,
    /// `\pagediscards` (0), `\splitdiscards` (1).
    Discards(u8),
    /// `\middle`.
    Middle,
    /// `\beginL`, `\endL`, `\beginR`, `\endR`: the math node's subtype
    /// (6, 7, 10, 11).
    Lr(u8),
    /// `\interlinepenalties` (0), `\clubpenalties`, `\widowpenalties`,
    /// `\displaywidowpenalties` (3).
    PenShape(u8),
}

/// The first of e-TeX's `LastItem` codes (`\eTeXversion`); the others
/// follow in `ETEX_PRIMITIVES`' order of codes: 6 `\currentgrouplevel`, 7
/// `\currentgrouptype`, 8 `\currentiflevel`, 9 `\currentiftype`, 10
/// `\currentifbranch`, 11 `\lastnodetype`, 12 `\gluestretchorder`, 13
/// `\glueshrinkorder`, 14–17 `\fontcharwd/ht/dp/ic`, 18–20
/// `\parshapelength/indent/dimen`, 21 `\gluestretch`, 22 `\glueshrink`,
/// 23 `\mutoglue`, 24 `\gluetomu`, 25–28 `\numexpr`, `\dimexpr`,
/// `\glueexpr`, `\muexpr`.
pub const ETEX_ITEM: u8 = 5;

impl Prim {
    /// Whether it is expanded (a step) rather than executed (tex.web 366).
    pub fn expandable(self) -> bool {
        matches!(
            self,
            Prim::If(_)
                | Prim::Fi
                | Prim::Else
                | Prim::Or
                | Prim::ExpandAfter
                | Prim::NoExpand
                | Prim::CsName
                | Prim::Convert(_)
                | Prim::The
                | Prim::Input
                | Prim::EndInput
                | Prim::Mark(_)
                | Prim::Unless
                | Prim::Unexpanded
                | Prim::Detokenize
                | Prim::ScanTokens
        )
    }

    /// Whether it is a conditional or a conditional's end.
    pub fn conditional(self) -> bool {
        matches!(self, Prim::If(_) | Prim::Fi | Prim::Else | Prim::Or | Prim::Unless)
    }

    pub fn by_name(n: &str) -> Option<Prim> {
        PRIMITIVES.binary_search_by(|(m, _, _)| (*m).cmp(n)).ok().map(|i| PRIMITIVES[i].1)
    }

    /// e-TeX's primitive of that name (extended mode only).
    pub fn by_name_etex(n: &str) -> Option<Prim> {
        ETEX_PRIMITIVES.binary_search_by(|(m, _)| (*m).cmp(n)).ok().map(|i| ETEX_PRIMITIVES[i].1)
    }

    /// The reader's internal commands, by the names only it makes (never
    /// a name a document can spell: they begin with a NUL).
    pub fn internal(n: &str) -> Option<Prim> {
        INTERNAL.iter().find(|(m, _)| *m == n).map(|(_, p)| *p)
    }

    /// Its name (`\meaning` prints it after the escape character).
    pub fn name(self) -> &'static str {
        if let Prim::Unimpl(i) = self {
            return UNIMPL[usize::from(i)];
        }
        if let Some((n, _)) = INTERNAL.iter().find(|(_, p)| *p == self) {
            return n;
        }
        PRIMITIVES
            .iter()
            .find(|(_, p, _)| *p == self)
            .map(|(n, _, _)| *n)
            .or_else(|| ETEX_PRIMITIVES.iter().find(|(_, p)| *p == self).map(|(n, _)| *n))
            .unwrap_or("IMPOSSIBLE.")
    }
}

impl IntPar {
    /// IniTeX's value (tex.web 240; the date and time as
    /// `SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1` make them).
    pub fn initial(self) -> i32 {
        match self {
            IntPar::Mag => 1000,
            IntPar::Tolerance => 10000,
            IntPar::Hangafter | IntPar::Month | IntPar::Day => 1,
            IntPar::Maxdeadcycles => 25,
            IntPar::Escapechar => 92,
            IntPar::Endlinechar => 13,
            IntPar::Year => 1970,
            _ => 0,
        }
    }
}

impl DimenPar {
    /// IniTeX's value: zero.
    pub fn initial(self) -> i32 {
        0
    }
}

impl GluePar {
    pub fn is_mu(self) -> bool {
        matches!(self, GluePar::Thinmuskip | GluePar::Medmuskip | GluePar::Thickmuskip)
    }
    /// IniTeX's value: zero glue.
    pub fn initial_width(self) -> i32 {
        0
    }
}

params! { IntPar, 65, Pretolerance = "pretolerance", Tolerance = "tolerance", Linepenalty = "linepenalty", Hyphenpenalty = "hyphenpenalty", Exhyphenpenalty = "exhyphenpenalty", Clubpenalty = "clubpenalty", Widowpenalty = "widowpenalty", Displaywidowpenalty = "displaywidowpenalty", Brokenpenalty = "brokenpenalty", Binoppenalty = "binoppenalty", Relpenalty = "relpenalty", Predisplaypenalty = "predisplaypenalty", Postdisplaypenalty = "postdisplaypenalty", Interlinepenalty = "interlinepenalty", Doublehyphendemerits = "doublehyphendemerits", Finalhyphendemerits = "finalhyphendemerits", Adjdemerits = "adjdemerits", Mag = "mag", Delimiterfactor = "delimiterfactor", Looseness = "looseness", Time = "time", Day = "day", Month = "month", Year = "year", Showboxbreadth = "showboxbreadth", Showboxdepth = "showboxdepth", Hbadness = "hbadness", Vbadness = "vbadness", Pausing = "pausing", Tracingonline = "tracingonline", Tracingmacros = "tracingmacros", Tracingstats = "tracingstats", Tracingparagraphs = "tracingparagraphs", Tracingpages = "tracingpages", Tracingoutput = "tracingoutput", Tracinglostchars = "tracinglostchars", Tracingcommands = "tracingcommands", Tracingrestores = "tracingrestores", Uchyph = "uchyph", Outputpenalty = "outputpenalty", Maxdeadcycles = "maxdeadcycles", Hangafter = "hangafter", Floatingpenalty = "floatingpenalty", Globaldefs = "globaldefs", Fam = "fam", Escapechar = "escapechar", Defaulthyphenchar = "defaulthyphenchar", Defaultskewchar = "defaultskewchar", Endlinechar = "endlinechar", Newlinechar = "newlinechar", Language = "language", Lefthyphenmin = "lefthyphenmin", Righthyphenmin = "righthyphenmin", Holdinginserts = "holdinginserts", Errorcontextlines = "errorcontextlines", Tracingassigns = "tracingassigns", Tracinggroups = "tracinggroups", Tracingifs = "tracingifs", Tracingscantokens = "tracingscantokens", Tracingnesting = "tracingnesting", Predisplaydirection = "predisplaydirection", Lastlinefit = "lastlinefit", Savingvdiscards = "savingvdiscards", Savinghyphcodes = "savinghyphcodes", TeXXeTstate = "TeXXeTstate" }
params! { DimenPar, 21, Parindent = "parindent", Mathsurround = "mathsurround", Lineskiplimit = "lineskiplimit", Hsize = "hsize", Vsize = "vsize", Maxdepth = "maxdepth", Splitmaxdepth = "splitmaxdepth", Boxmaxdepth = "boxmaxdepth", Hfuzz = "hfuzz", Vfuzz = "vfuzz", Delimitershortfall = "delimitershortfall", Nulldelimiterspace = "nulldelimiterspace", Scriptspace = "scriptspace", Predisplaysize = "predisplaysize", Displaywidth = "displaywidth", Displayindent = "displayindent", Overfullrule = "overfullrule", Hangindent = "hangindent", Hoffset = "hoffset", Voffset = "voffset", Emergencystretch = "emergencystretch" }
params! { GluePar, 18, Lineskip = "lineskip", Baselineskip = "baselineskip", Parskip = "parskip", Abovedisplayskip = "abovedisplayskip", Belowdisplayskip = "belowdisplayskip", Abovedisplayshortskip = "abovedisplayshortskip", Belowdisplayshortskip = "belowdisplayshortskip", Leftskip = "leftskip", Rightskip = "rightskip", Topskip = "topskip", Splittopskip = "splittopskip", Tabskip = "tabskip", Spaceskip = "spaceskip", Xspaceskip = "xspaceskip", Parfillskip = "parfillskip", Thinmuskip = "thinmuskip", Medmuskip = "medmuskip", Thickmuskip = "thickmuskip" }
params! { TokPar, 10, Output = "output", Everypar = "everypar", Everymath = "everymath", Everydisplay = "everydisplay", Everyhbox = "everyhbox", Everyvbox = "everyvbox", Everyjob = "everyjob", Everycr = "everycr", Errhelp = "errhelp", Everyeof = "everyeof" }

/// The reader's internal commands (`Mat::Internal`).
pub const INTERNAL: [(&str, Prim); 9] = [
    ("\u{0}package", Prim::Mat(Mat::Internal(0))),
    ("\u{0}endgraf", Prim::Mat(Mat::Internal(1))),
    ("\u{0}discnext", Prim::Mat(Mat::Internal(2))),
    ("\u{0}normalparagraph", Prim::Mat(Mat::Internal(3))),
    ("\u{0}mathinit", Prim::Mat(Mat::Internal(4))),
    ("\u{0}display", Prim::Mat(Mat::Internal(5))),
    ("\u{0}displayend", Prim::Mat(Mat::Internal(6))),
    ("\u{0}mathgroup", Prim::Mat(Mat::Internal(7))),
    ("\u{0}choicenext", Prim::Mat(Mat::Internal(8))),
];

/// Every TeX82 primitive (tex.web §1336: partex-core's `init_prim_tex` less
/// encTeX's 14), sorted by name: its name, its meaning here, and its
/// command code in tex.web.
pub const PRIMITIVES: [(&str, Prim, &str); 325] = [
    (" ", Prim::Space, "EX_SPACE"),
    ("-", Prim::Mat(Mat::Disc(false)), "DISCRETIONARY"),
    ("/", Prim::Mat(Mat::ItalCorr), "ITAL_CORR"),
    ("above", Prim::Math(MathP::Above(0)), "ABOVE"),
    ("abovedisplayshortskip", Prim::GluePar(GluePar::Abovedisplayshortskip), "ASSIGN_GLUE"),
    ("abovedisplayskip", Prim::GluePar(GluePar::Abovedisplayskip), "ASSIGN_GLUE"),
    ("abovewithdelims", Prim::Math(MathP::Above(3)), "ABOVE"),
    ("accent", Prim::Mat(Mat::Accent), "ACCENT"),
    ("adjdemerits", Prim::IntPar(IntPar::Adjdemerits), "ASSIGN_INT"),
    ("advance", Prim::Advance, "ADVANCE"),
    ("afterassignment", Prim::AfterAssignment, "AFTER_ASSIGNMENT"),
    ("aftergroup", Prim::AfterGroup, "AFTER_GROUP"),
    ("atop", Prim::Math(MathP::Above(2)), "ABOVE"),
    ("atopwithdelims", Prim::Math(MathP::Above(5)), "ABOVE"),
    ("badness", Prim::LastItem(4), "LAST_ITEM"),
    ("baselineskip", Prim::GluePar(GluePar::Baselineskip), "ASSIGN_GLUE"),
    ("batchmode", Prim::Interaction(0), "SET_INTERACTION"),
    ("begingroup", Prim::BeginGroup, "BEGIN_GROUP"),
    ("belowdisplayshortskip", Prim::GluePar(GluePar::Belowdisplayshortskip), "ASSIGN_GLUE"),
    ("belowdisplayskip", Prim::GluePar(GluePar::Belowdisplayskip), "ASSIGN_GLUE"),
    ("binoppenalty", Prim::IntPar(IntPar::Binoppenalty), "ASSIGN_INT"),
    ("botmark", Prim::Mark(2), "TOP_BOT_MARK"),
    ("box", Prim::Mat(Mat::MakeBox(0)), "MAKE_BOX"),
    ("boxmaxdepth", Prim::DimenPar(DimenPar::Boxmaxdepth), "ASSIGN_DIMEN"),
    ("brokenpenalty", Prim::IntPar(IntPar::Brokenpenalty), "ASSIGN_INT"),
    ("catcode", Prim::Code(Code::Cat), "DEF_CODE"),
    ("char", Prim::Char, "CHAR_NUM"),
    ("chardef", Prim::Shorthand(Shorthand::Char), "SHORTHAND_DEF"),
    ("cleaders", Prim::Mat(Mat::Leaders(101)), "LEADER_SHIP"),
    ("closein", Prim::InStream(false), "IN_STREAM"),
    ("closeout", Prim::CloseOut, "EXTENSION"),
    ("clubpenalty", Prim::IntPar(IntPar::Clubpenalty), "ASSIGN_INT"),
    ("copy", Prim::Mat(Mat::MakeBox(1)), "MAKE_BOX"),
    ("count", Prim::Register(Reg::Count), "REGISTER"),
    ("countdef", Prim::Shorthand(Shorthand::Count), "SHORTHAND_DEF"),
    ("cr", Prim::Unimpl(24), "CAR_RET"),
    ("crcr", Prim::Unimpl(25), "CAR_RET"),
    ("csname", Prim::CsName, "CS_NAME"),
    ("day", Prim::IntPar(IntPar::Day), "ASSIGN_INT"),
    ("deadcycles", Prim::PageInt(0), "SET_PAGE_INT"),
    ("def", Prim::Def, "DEF"),
    ("defaulthyphenchar", Prim::IntPar(IntPar::Defaulthyphenchar), "ASSIGN_INT"),
    ("defaultskewchar", Prim::IntPar(IntPar::Defaultskewchar), "ASSIGN_INT"),
    ("delcode", Prim::Code(Code::Del), "DEF_CODE"),
    ("delimiter", Prim::Math(MathP::Delimiter), "DELIM_NUM"),
    ("delimiterfactor", Prim::IntPar(IntPar::Delimiterfactor), "ASSIGN_INT"),
    ("delimitershortfall", Prim::DimenPar(DimenPar::Delimitershortfall), "ASSIGN_DIMEN"),
    ("dimen", Prim::Register(Reg::Dimen), "REGISTER"),
    ("dimendef", Prim::Shorthand(Shorthand::Dimen), "SHORTHAND_DEF"),
    ("discretionary", Prim::Mat(Mat::Disc(true)), "DISCRETIONARY"),
    ("displayindent", Prim::DimenPar(DimenPar::Displayindent), "ASSIGN_DIMEN"),
    ("displaylimits", Prim::Math(MathP::Limits(0)), "LIMIT_SWITCH"),
    ("displaystyle", Prim::Math(MathP::Style(0)), "MATH_STYLE"),
    ("displaywidowpenalty", Prim::IntPar(IntPar::Displaywidowpenalty), "ASSIGN_INT"),
    ("displaywidth", Prim::DimenPar(DimenPar::Displaywidth), "ASSIGN_DIMEN"),
    ("divide", Prim::Divide, "DIVIDE"),
    ("doublehyphendemerits", Prim::IntPar(IntPar::Doublehyphendemerits), "ASSIGN_INT"),
    ("dp", Prim::BoxDimen(2), "SET_BOX_DIMEN"),
    ("dump", Prim::Dump, "STOP"),
    ("edef", Prim::Edef, "DEF"),
    ("else", Prim::Else, "FI_OR_ELSE"),
    ("emergencystretch", Prim::DimenPar(DimenPar::Emergencystretch), "ASSIGN_DIMEN"),
    ("end", Prim::End, "STOP"),
    ("endcsname", Prim::EndCsName, "END_CS_NAME"),
    ("endgroup", Prim::EndGroup, "END_GROUP"),
    ("endinput", Prim::EndInput, "INPUT"),
    ("endlinechar", Prim::IntPar(IntPar::Endlinechar), "ASSIGN_INT"),
    ("eqno", Prim::Math(MathP::EqNo(false)), "EQ_NO"),
    ("errhelp", Prim::TokPar(TokPar::Errhelp), "ASSIGN_TOKS"),
    ("errmessage", Prim::ErrMessage, "MESSAGE"),
    ("errorcontextlines", Prim::IntPar(IntPar::Errorcontextlines), "ASSIGN_INT"),
    ("errorstopmode", Prim::Interaction(3), "SET_INTERACTION"),
    ("escapechar", Prim::IntPar(IntPar::Escapechar), "ASSIGN_INT"),
    ("everycr", Prim::TokPar(TokPar::Everycr), "ASSIGN_TOKS"),
    ("everydisplay", Prim::TokPar(TokPar::Everydisplay), "ASSIGN_TOKS"),
    ("everyhbox", Prim::TokPar(TokPar::Everyhbox), "ASSIGN_TOKS"),
    ("everyjob", Prim::TokPar(TokPar::Everyjob), "ASSIGN_TOKS"),
    ("everymath", Prim::TokPar(TokPar::Everymath), "ASSIGN_TOKS"),
    ("everypar", Prim::TokPar(TokPar::Everypar), "ASSIGN_TOKS"),
    ("everyvbox", Prim::TokPar(TokPar::Everyvbox), "ASSIGN_TOKS"),
    ("exhyphenpenalty", Prim::IntPar(IntPar::Exhyphenpenalty), "ASSIGN_INT"),
    ("expandafter", Prim::ExpandAfter, "EXPAND_AFTER"),
    ("fam", Prim::IntPar(IntPar::Fam), "ASSIGN_INT"),
    ("fi", Prim::Fi, "FI_OR_ELSE"),
    ("finalhyphendemerits", Prim::IntPar(IntPar::Finalhyphendemerits), "ASSIGN_INT"),
    ("firstmark", Prim::Mark(1), "TOP_BOT_MARK"),
    ("floatingpenalty", Prim::IntPar(IntPar::Floatingpenalty), "ASSIGN_INT"),
    ("font", Prim::Font, "DEF_FONT"),
    ("fontdimen", Prim::FontDimen, "ASSIGN_FONT_DIMEN"),
    ("fontname", Prim::Convert(Conv::FontName), "CONVERT"),
    ("futurelet", Prim::Futurelet, "LET"),
    ("gdef", Prim::Gdef, "DEF"),
    ("global", Prim::Global, "PREFIX"),
    ("globaldefs", Prim::IntPar(IntPar::Globaldefs), "ASSIGN_INT"),
    ("halign", Prim::Unimpl(3), "HALIGN"),
    ("hangafter", Prim::IntPar(IntPar::Hangafter), "ASSIGN_INT"),
    ("hangindent", Prim::DimenPar(DimenPar::Hangindent), "ASSIGN_DIMEN"),
    ("hbadness", Prim::IntPar(IntPar::Hbadness), "ASSIGN_INT"),
    ("hbox", Prim::Mat(Mat::MakeBox(6)), "MAKE_BOX"),
    ("hfil", Prim::Mat(Mat::Hskip(1)), "HSKIP"),
    ("hfill", Prim::Mat(Mat::Hskip(2)), "HSKIP"),
    ("hfilneg", Prim::Mat(Mat::Hskip(4)), "HSKIP"),
    ("hfuzz", Prim::DimenPar(DimenPar::Hfuzz), "ASSIGN_DIMEN"),
    ("hoffset", Prim::DimenPar(DimenPar::Hoffset), "ASSIGN_DIMEN"),
    ("holdinginserts", Prim::IntPar(IntPar::Holdinginserts), "ASSIGN_INT"),
    ("hrule", Prim::Mat(Mat::Hrule), "HRULE"),
    ("hsize", Prim::DimenPar(DimenPar::Hsize), "ASSIGN_DIMEN"),
    ("hskip", Prim::Mat(Mat::Hskip(0)), "HSKIP"),
    ("hss", Prim::Mat(Mat::Hskip(3)), "HSKIP"),
    ("ht", Prim::BoxDimen(1), "SET_BOX_DIMEN"),
    ("hyphenation", Prim::HyphData(false), "HYPH_DATA"),
    ("hyphenchar", Prim::Mat(Mat::FontInt(0)), "ASSIGN_FONT_INT"),
    ("hyphenpenalty", Prim::IntPar(IntPar::Hyphenpenalty), "ASSIGN_INT"),
    ("if", Prim::If(IfKind::Char), "IF_TEST"),
    ("ifcase", Prim::If(IfKind::Case), "IF_TEST"),
    ("ifcat", Prim::If(IfKind::Cat), "IF_TEST"),
    ("ifdim", Prim::If(IfKind::Dim), "IF_TEST"),
    ("ifeof", Prim::If(IfKind::Eof), "IF_TEST"),
    ("iffalse", Prim::If(IfKind::False), "IF_TEST"),
    ("ifhbox", Prim::If(IfKind::Hbox), "IF_TEST"),
    ("ifhmode", Prim::If(IfKind::Hmode), "IF_TEST"),
    ("ifinner", Prim::If(IfKind::Inner), "IF_TEST"),
    ("ifmmode", Prim::If(IfKind::Mmode), "IF_TEST"),
    ("ifnum", Prim::If(IfKind::Num), "IF_TEST"),
    ("ifodd", Prim::If(IfKind::Odd), "IF_TEST"),
    ("iftrue", Prim::If(IfKind::True), "IF_TEST"),
    ("ifvbox", Prim::If(IfKind::Vbox), "IF_TEST"),
    ("ifvmode", Prim::If(IfKind::Vmode), "IF_TEST"),
    ("ifvoid", Prim::If(IfKind::Void), "IF_TEST"),
    ("ifx", Prim::If(IfKind::X), "IF_TEST"),
    ("ignorespaces", Prim::IgnoreSpaces, "IGNORE_SPACES"),
    ("immediate", Prim::Immediate, "EXTENSION"),
    ("indent", Prim::Indent, "START_PAR"),
    ("input", Prim::Input, "INPUT"),
    ("inputlineno", Prim::LastItem(3), "LAST_ITEM"),
    ("insert", Prim::Mat(Mat::Insert), "INSERT"),
    ("insertpenalties", Prim::PageInt(1), "SET_PAGE_INT"),
    ("interlinepenalty", Prim::IntPar(IntPar::Interlinepenalty), "ASSIGN_INT"),
    ("jobname", Prim::Convert(Conv::JobName), "CONVERT"),
    ("kern", Prim::Mat(Mat::Kern), "KERN"),
    ("language", Prim::IntPar(IntPar::Language), "ASSIGN_INT"),
    ("lastbox", Prim::Mat(Mat::MakeBox(2)), "MAKE_BOX"),
    ("lastkern", Prim::LastItem(1), "LAST_ITEM"),
    ("lastpenalty", Prim::LastItem(0), "LAST_ITEM"),
    ("lastskip", Prim::LastItem(2), "LAST_ITEM"),
    ("lccode", Prim::Code(Code::Lc), "DEF_CODE"),
    ("leaders", Prim::Mat(Mat::Leaders(100)), "LEADER_SHIP"),
    ("left", Prim::Math(MathP::Left), "LEFT_RIGHT"),
    ("lefthyphenmin", Prim::IntPar(IntPar::Lefthyphenmin), "ASSIGN_INT"),
    ("leftskip", Prim::GluePar(GluePar::Leftskip), "ASSIGN_GLUE"),
    ("leqno", Prim::Math(MathP::EqNo(true)), "EQ_NO"),
    ("let", Prim::Let, "LET"),
    ("limits", Prim::Math(MathP::Limits(1)), "LIMIT_SWITCH"),
    ("linepenalty", Prim::IntPar(IntPar::Linepenalty), "ASSIGN_INT"),
    ("lineskip", Prim::GluePar(GluePar::Lineskip), "ASSIGN_GLUE"),
    ("lineskiplimit", Prim::DimenPar(DimenPar::Lineskiplimit), "ASSIGN_DIMEN"),
    ("long", Prim::Long, "PREFIX"),
    ("looseness", Prim::IntPar(IntPar::Looseness), "ASSIGN_INT"),
    ("lower", Prim::Mat(Mat::Vmove(true)), "VMOVE"),
    ("lowercase", Prim::Lowercase, "CASE_SHIFT"),
    ("mag", Prim::IntPar(IntPar::Mag), "ASSIGN_INT"),
    ("mark", Prim::Mat(Mat::Mark), "MARK"),
    ("mathaccent", Prim::Math(MathP::Accent), "MATH_ACCENT"),
    ("mathbin", Prim::Math(MathP::Comp(2)), "MATH_COMP"),
    ("mathchar", Prim::Math(MathP::MathChar), "MATH_CHAR_NUM"),
    ("mathchardef", Prim::Shorthand(Shorthand::MathChar), "SHORTHAND_DEF"),
    ("mathchoice", Prim::Math(MathP::Choice), "MATH_CHOICE"),
    ("mathclose", Prim::Math(MathP::Comp(5)), "MATH_COMP"),
    ("mathcode", Prim::Code(Code::Math), "DEF_CODE"),
    ("mathinner", Prim::Math(MathP::Comp(7)), "MATH_COMP"),
    ("mathop", Prim::Math(MathP::Comp(1)), "MATH_COMP"),
    ("mathopen", Prim::Math(MathP::Comp(4)), "MATH_COMP"),
    ("mathord", Prim::Math(MathP::Comp(0)), "MATH_COMP"),
    ("mathpunct", Prim::Math(MathP::Comp(6)), "MATH_COMP"),
    ("mathrel", Prim::Math(MathP::Comp(3)), "MATH_COMP"),
    ("mathsurround", Prim::DimenPar(DimenPar::Mathsurround), "ASSIGN_DIMEN"),
    ("maxdeadcycles", Prim::IntPar(IntPar::Maxdeadcycles), "ASSIGN_INT"),
    ("maxdepth", Prim::DimenPar(DimenPar::Maxdepth), "ASSIGN_DIMEN"),
    ("meaning", Prim::Convert(Conv::Meaning), "CONVERT"),
    ("medmuskip", Prim::GluePar(GluePar::Medmuskip), "ASSIGN_MU_GLUE"),
    ("message", Prim::Message, "MESSAGE"),
    ("mkern", Prim::Math(MathP::Mkern), "MKERN"),
    ("month", Prim::IntPar(IntPar::Month), "ASSIGN_INT"),
    ("moveleft", Prim::Mat(Mat::Hmove(true)), "HMOVE"),
    ("moveright", Prim::Mat(Mat::Hmove(false)), "HMOVE"),
    ("mskip", Prim::Math(MathP::Mskip), "MSKIP"),
    ("multiply", Prim::Multiply, "MULTIPLY"),
    ("muskip", Prim::Register(Reg::Muskip), "REGISTER"),
    ("muskipdef", Prim::Shorthand(Shorthand::Muskip), "SHORTHAND_DEF"),
    ("newlinechar", Prim::IntPar(IntPar::Newlinechar), "ASSIGN_INT"),
    ("noalign", Prim::Unimpl(10), "NO_ALIGN"),
    ("noboundary", Prim::Unimpl(11), "NO_BOUNDARY"),
    ("noexpand", Prim::NoExpand, "NO_EXPAND"),
    ("noindent", Prim::Noindent, "START_PAR"),
    ("nolimits", Prim::Math(MathP::Limits(2)), "LIMIT_SWITCH"),
    ("nonscript", Prim::Math(MathP::NonScript), "NON_SCRIPT"),
    ("nonstopmode", Prim::Interaction(1), "SET_INTERACTION"),
    ("nulldelimiterspace", Prim::DimenPar(DimenPar::Nulldelimiterspace), "ASSIGN_DIMEN"),
    ("nullfont", Prim::Unimpl(22), "SET_FONT"),
    ("number", Prim::Convert(Conv::Number), "CONVERT"),
    ("omit", Prim::Unimpl(13), "OMIT"),
    ("openin", Prim::InStream(true), "IN_STREAM"),
    ("openout", Prim::OpenOut, "EXTENSION"),
    ("or", Prim::Or, "FI_OR_ELSE"),
    ("outer", Prim::Outer, "PREFIX"),
    ("output", Prim::TokPar(TokPar::Output), "ASSIGN_TOKS"),
    ("outputpenalty", Prim::IntPar(IntPar::Outputpenalty), "ASSIGN_INT"),
    ("over", Prim::Math(MathP::Above(1)), "ABOVE"),
    ("overfullrule", Prim::DimenPar(DimenPar::Overfullrule), "ASSIGN_DIMEN"),
    ("overline", Prim::Math(MathP::Comp(9)), "MATH_COMP"),
    ("overwithdelims", Prim::Math(MathP::Above(4)), "ABOVE"),
    ("pagedepth", Prim::PageDimen(7), "SET_PAGE_DIMEN"),
    ("pagefilllstretch", Prim::PageDimen(5), "SET_PAGE_DIMEN"),
    ("pagefillstretch", Prim::PageDimen(4), "SET_PAGE_DIMEN"),
    ("pagefilstretch", Prim::PageDimen(3), "SET_PAGE_DIMEN"),
    ("pagegoal", Prim::PageDimen(0), "SET_PAGE_DIMEN"),
    ("pageshrink", Prim::PageDimen(6), "SET_PAGE_DIMEN"),
    ("pagestretch", Prim::PageDimen(2), "SET_PAGE_DIMEN"),
    ("pagetotal", Prim::PageDimen(1), "SET_PAGE_DIMEN"),
    ("par", Prim::Par, "PAR_END"),
    ("parfillskip", Prim::GluePar(GluePar::Parfillskip), "ASSIGN_GLUE"),
    ("parindent", Prim::DimenPar(DimenPar::Parindent), "ASSIGN_DIMEN"),
    ("parshape", Prim::Parshape, "SET_SHAPE"),
    ("parskip", Prim::GluePar(GluePar::Parskip), "ASSIGN_GLUE"),
    ("patterns", Prim::HyphData(true), "HYPH_DATA"),
    ("pausing", Prim::IntPar(IntPar::Pausing), "ASSIGN_INT"),
    ("penalty", Prim::Mat(Mat::Penalty), "BREAK_PENALTY"),
    ("postdisplaypenalty", Prim::IntPar(IntPar::Postdisplaypenalty), "ASSIGN_INT"),
    ("predisplaypenalty", Prim::IntPar(IntPar::Predisplaypenalty), "ASSIGN_INT"),
    ("predisplaysize", Prim::DimenPar(DimenPar::Predisplaysize), "ASSIGN_DIMEN"),
    ("pretolerance", Prim::IntPar(IntPar::Pretolerance), "ASSIGN_INT"),
    ("prevdepth", Prim::PrevDepth, "SET_AUX"),
    ("prevgraf", Prim::PrevGraf, "SET_PREV_GRAF"),
    ("radical", Prim::Math(MathP::Radical), "RADICAL"),
    ("raise", Prim::Mat(Mat::Vmove(false)), "VMOVE"),
    ("read", Prim::Read, "READ_TO_CS"),
    ("relax", Prim::Relax, "RELAX"),
    ("relpenalty", Prim::IntPar(IntPar::Relpenalty), "ASSIGN_INT"),
    ("right", Prim::Math(MathP::Right), "LEFT_RIGHT"),
    ("righthyphenmin", Prim::IntPar(IntPar::Righthyphenmin), "ASSIGN_INT"),
    ("rightskip", Prim::GluePar(GluePar::Rightskip), "ASSIGN_GLUE"),
    ("romannumeral", Prim::Convert(Conv::Roman), "CONVERT"),
    ("scriptfont", Prim::DefFamily(1), "DEF_FAMILY"),
    ("scriptscriptfont", Prim::DefFamily(2), "DEF_FAMILY"),
    ("scriptscriptstyle", Prim::Math(MathP::Style(6)), "MATH_STYLE"),
    ("scriptspace", Prim::DimenPar(DimenPar::Scriptspace), "ASSIGN_DIMEN"),
    ("scriptstyle", Prim::Math(MathP::Style(4)), "MATH_STYLE"),
    ("scrollmode", Prim::Interaction(2), "SET_INTERACTION"),
    ("setbox", Prim::Mat(Mat::SetBox), "SET_BOX"),
    ("setlanguage", Prim::SetLanguage, "EXTENSION"),
    ("sfcode", Prim::Code(Code::Sf), "DEF_CODE"),
    ("shipout", Prim::Mat(Mat::Shipout), "LEADER_SHIP"),
    ("show", Prim::Unimpl(99), "XRAY"),
    ("showbox", Prim::Show(1), "XRAY"),
    ("showboxbreadth", Prim::IntPar(IntPar::Showboxbreadth), "ASSIGN_INT"),
    ("showboxdepth", Prim::IntPar(IntPar::Showboxdepth), "ASSIGN_INT"),
    ("showlists", Prim::Show(3), "XRAY"),
    ("showthe", Prim::Unimpl(101), "XRAY"),
    ("skewchar", Prim::Mat(Mat::FontInt(1)), "ASSIGN_FONT_INT"),
    ("skip", Prim::Register(Reg::Skip), "REGISTER"),
    ("skipdef", Prim::Shorthand(Shorthand::Skip), "SHORTHAND_DEF"),
    ("spacefactor", Prim::SpaceFactor, "SET_AUX"),
    ("spaceskip", Prim::GluePar(GluePar::Spaceskip), "ASSIGN_GLUE"),
    ("span", Prim::Unimpl(23), "TAB_MARK"),
    ("special", Prim::Special, "EXTENSION"),
    ("splitbotmark", Prim::Mark(4), "TOP_BOT_MARK"),
    ("splitfirstmark", Prim::Mark(3), "TOP_BOT_MARK"),
    ("splitmaxdepth", Prim::DimenPar(DimenPar::Splitmaxdepth), "ASSIGN_DIMEN"),
    ("splittopskip", Prim::GluePar(GluePar::Splittopskip), "ASSIGN_GLUE"),
    ("string", Prim::Convert(Conv::String), "CONVERT"),
    ("tabskip", Prim::GluePar(GluePar::Tabskip), "ASSIGN_GLUE"),
    ("textfont", Prim::DefFamily(0), "DEF_FAMILY"),
    ("textstyle", Prim::Math(MathP::Style(2)), "MATH_STYLE"),
    ("the", Prim::The, "THE"),
    ("thickmuskip", Prim::GluePar(GluePar::Thickmuskip), "ASSIGN_MU_GLUE"),
    ("thinmuskip", Prim::GluePar(GluePar::Thinmuskip), "ASSIGN_MU_GLUE"),
    ("time", Prim::IntPar(IntPar::Time), "ASSIGN_INT"),
    ("toks", Prim::Register(Reg::Toks), "TOKS_REGISTER"),
    ("toksdef", Prim::Shorthand(Shorthand::Toks), "SHORTHAND_DEF"),
    ("tolerance", Prim::IntPar(IntPar::Tolerance), "ASSIGN_INT"),
    ("topmark", Prim::Mark(0), "TOP_BOT_MARK"),
    ("topskip", Prim::GluePar(GluePar::Topskip), "ASSIGN_GLUE"),
    ("tracingcommands", Prim::IntPar(IntPar::Tracingcommands), "ASSIGN_INT"),
    ("tracinglostchars", Prim::IntPar(IntPar::Tracinglostchars), "ASSIGN_INT"),
    ("tracingmacros", Prim::IntPar(IntPar::Tracingmacros), "ASSIGN_INT"),
    ("tracingonline", Prim::IntPar(IntPar::Tracingonline), "ASSIGN_INT"),
    ("tracingoutput", Prim::IntPar(IntPar::Tracingoutput), "ASSIGN_INT"),
    ("tracingpages", Prim::IntPar(IntPar::Tracingpages), "ASSIGN_INT"),
    ("tracingparagraphs", Prim::IntPar(IntPar::Tracingparagraphs), "ASSIGN_INT"),
    ("tracingrestores", Prim::IntPar(IntPar::Tracingrestores), "ASSIGN_INT"),
    ("tracingstats", Prim::IntPar(IntPar::Tracingstats), "ASSIGN_INT"),
    ("uccode", Prim::Code(Code::Uc), "DEF_CODE"),
    ("uchyph", Prim::IntPar(IntPar::Uchyph), "ASSIGN_INT"),
    ("underline", Prim::Math(MathP::Comp(8)), "MATH_COMP"),
    ("unhbox", Prim::Mat(Mat::Unhbox(false)), "UN_HBOX"),
    ("unhcopy", Prim::Mat(Mat::Unhbox(true)), "UN_HBOX"),
    ("unkern", Prim::Mat(Mat::Remove(1)), "REMOVE_ITEM"),
    ("unpenalty", Prim::Mat(Mat::Remove(0)), "REMOVE_ITEM"),
    ("unskip", Prim::Mat(Mat::Remove(2)), "REMOVE_ITEM"),
    ("unvbox", Prim::Mat(Mat::Unvbox(false)), "UN_VBOX"),
    ("unvcopy", Prim::Mat(Mat::Unvbox(true)), "UN_VBOX"),
    ("uppercase", Prim::Uppercase, "CASE_SHIFT"),
    ("vadjust", Prim::Mat(Mat::Vadjust), "VADJUST"),
    ("valign", Prim::Unimpl(19), "VALIGN"),
    ("vbadness", Prim::IntPar(IntPar::Vbadness), "ASSIGN_INT"),
    ("vbox", Prim::Mat(Mat::MakeBox(5)), "MAKE_BOX"),
    ("vcenter", Prim::Math(MathP::Vcenter), "VCENTER"),
    ("vfil", Prim::Mat(Mat::Vskip(1)), "VSKIP"),
    ("vfill", Prim::Mat(Mat::Vskip(2)), "VSKIP"),
    ("vfilneg", Prim::Mat(Mat::Vskip(4)), "VSKIP"),
    ("vfuzz", Prim::DimenPar(DimenPar::Vfuzz), "ASSIGN_DIMEN"),
    ("voffset", Prim::DimenPar(DimenPar::Voffset), "ASSIGN_DIMEN"),
    ("vrule", Prim::Mat(Mat::Vrule), "VRULE"),
    ("vsize", Prim::DimenPar(DimenPar::Vsize), "ASSIGN_DIMEN"),
    ("vskip", Prim::Mat(Mat::Vskip(0)), "VSKIP"),
    ("vsplit", Prim::Mat(Mat::MakeBox(3)), "MAKE_BOX"),
    ("vss", Prim::Mat(Mat::Vskip(3)), "VSKIP"),
    ("vtop", Prim::Mat(Mat::MakeBox(4)), "MAKE_BOX"),
    ("wd", Prim::BoxDimen(0), "SET_BOX_DIMEN"),
    ("widowpenalty", Prim::IntPar(IntPar::Widowpenalty), "ASSIGN_INT"),
    ("write", Prim::Write, "EXTENSION"),
    ("xdef", Prim::Xdef, "DEF"),
    ("xleaders", Prim::Mat(Mat::Leaders(102)), "LEADER_SHIP"),
    ("xspaceskip", Prim::GluePar(GluePar::Xspaceskip), "ASSIGN_GLUE"),
    ("year", Prim::IntPar(IntPar::Year), "ASSIGN_INT"),
];

/// The primitives without a graph form yet (`Prim::Unimpl(i)` is `UNIMPL[i]`).
pub const UNIMPL: [&str; 107] = [
    "/",
    "accent",
    "delimiter",
    "halign",
    "hrule",
    "insert",
    "mark",
    "mathaccent",
    "mathchar",
    "mathchoice",
    "noalign",
    "noboundary",
    "nonscript",
    "omit",
    "penalty",
    "radical",
    "read",
    "setbox",
    "vadjust",
    "valign",
    "vcenter",
    "vrule",
    "nullfont",
    "span",
    "cr",
    "crcr",
    "hskip",
    "hfil",
    "hfill",
    "hss",
    "hfilneg",
    "vskip",
    "vfil",
    "vfill",
    "vss",
    "vfilneg",
    "mskip",
    "kern",
    "mkern",
    "moveleft",
    "moveright",
    "raise",
    "lower",
    "box",
    "copy",
    "lastbox",
    "vsplit",
    "vtop",
    "vbox",
    "hbox",
    "shipout",
    "leaders",
    "cleaders",
    "xleaders",
    "unpenalty",
    "unkern",
    "unskip",
    "unhbox",
    "unhcopy",
    "unvbox",
    "unvcopy",
    "-",
    "discretionary",
    "eqno",
    "leqno",
    "mathord",
    "mathop",
    "mathbin",
    "mathrel",
    "mathopen",
    "mathclose",
    "mathpunct",
    "mathinner",
    "underline",
    "overline",
    "displaylimits",
    "limits",
    "nolimits",
    "displaystyle",
    "textstyle",
    "scriptstyle",
    "scriptscriptstyle",
    "above",
    "over",
    "atop",
    "abovewithdelims",
    "overwithdelims",
    "atopwithdelims",
    "left",
    "right",
    "textfont",
    "scriptfont",
    "scriptscriptfont",
    "hyphenation",
    "patterns",
    "hyphenchar",
    "skewchar",
    "openin",
    "closein",
    "show",
    "showbox",
    "showthe",
    "showlists",
    "openout",
    "closeout",
    "special",
    "setlanguage",
];

/// e-TeX's primitives (etex.web §1380, `generate_eTeX_prims`), sorted by
/// name (byte order): named only in extended mode (`Options::etex`).
pub const ETEX_PRIMITIVES: [(&str, Prim); 66] = [
    ("TeXXeTstate", Prim::IntPar(IntPar::TeXXeTstate)),
    ("beginL", Prim::Lr(6)),
    ("beginR", Prim::Lr(10)),
    ("botmarks", Prim::Mark(7)),
    ("clubpenalties", Prim::PenShape(1)),
    ("currentgrouplevel", Prim::LastItem(6)),
    ("currentgrouptype", Prim::LastItem(7)),
    ("currentifbranch", Prim::LastItem(10)),
    ("currentiflevel", Prim::LastItem(8)),
    ("currentiftype", Prim::LastItem(9)),
    ("detokenize", Prim::Detokenize),
    ("dimexpr", Prim::LastItem(26)),
    ("displaywidowpenalties", Prim::PenShape(3)),
    ("eTeXrevision", Prim::Convert(Conv::ETeXRevision)),
    ("eTeXversion", Prim::LastItem(5)),
    ("endL", Prim::Lr(7)),
    ("endR", Prim::Lr(11)),
    ("everyeof", Prim::TokPar(TokPar::Everyeof)),
    ("firstmarks", Prim::Mark(6)),
    ("fontchardp", Prim::LastItem(16)),
    ("fontcharht", Prim::LastItem(15)),
    ("fontcharic", Prim::LastItem(17)),
    ("fontcharwd", Prim::LastItem(14)),
    ("glueexpr", Prim::LastItem(27)),
    ("glueshrink", Prim::LastItem(22)),
    ("glueshrinkorder", Prim::LastItem(13)),
    ("gluestretch", Prim::LastItem(21)),
    ("gluestretchorder", Prim::LastItem(12)),
    ("gluetomu", Prim::LastItem(24)),
    ("ifcsname", Prim::If(IfKind::Csname)),
    ("ifdefined", Prim::If(IfKind::Defined)),
    ("iffontchar", Prim::If(IfKind::FontChar)),
    ("interactionmode", Prim::PageInt(2)),
    ("interlinepenalties", Prim::PenShape(0)),
    ("lastlinefit", Prim::IntPar(IntPar::Lastlinefit)),
    ("lastnodetype", Prim::LastItem(11)),
    ("marks", Prim::Marks),
    ("middle", Prim::Middle),
    ("muexpr", Prim::LastItem(28)),
    ("mutoglue", Prim::LastItem(23)),
    ("numexpr", Prim::LastItem(25)),
    ("pagediscards", Prim::Discards(0)),
    ("parshapedimen", Prim::LastItem(20)),
    ("parshapeindent", Prim::LastItem(19)),
    ("parshapelength", Prim::LastItem(18)),
    ("predisplaydirection", Prim::IntPar(IntPar::Predisplaydirection)),
    ("protected", Prim::Protected),
    ("readline", Prim::ReadLine),
    ("savinghyphcodes", Prim::IntPar(IntPar::Savinghyphcodes)),
    ("savingvdiscards", Prim::IntPar(IntPar::Savingvdiscards)),
    ("scantokens", Prim::ScanTokens),
    ("showgroups", Prim::ShowE(4)),
    ("showifs", Prim::ShowE(6)),
    ("showtokens", Prim::ShowE(5)),
    ("splitbotmarks", Prim::Mark(9)),
    ("splitdiscards", Prim::Discards(1)),
    ("splitfirstmarks", Prim::Mark(8)),
    ("topmarks", Prim::Mark(5)),
    ("tracingassigns", Prim::IntPar(IntPar::Tracingassigns)),
    ("tracinggroups", Prim::IntPar(IntPar::Tracinggroups)),
    ("tracingifs", Prim::IntPar(IntPar::Tracingifs)),
    ("tracingnesting", Prim::IntPar(IntPar::Tracingnesting)),
    ("tracingscantokens", Prim::IntPar(IntPar::Tracingscantokens)),
    ("unexpanded", Prim::Unexpanded),
    ("unless", Prim::Unless),
    ("widowpenalties", Prim::PenShape(2)),
];

#[cfg(test)]
mod etex_tests {
    use super::*;

    #[test]
    fn etex_table_is_sorted_and_named_back() {
        for w in ETEX_PRIMITIVES.windows(2) {
            assert!(w[0].0 < w[1].0, "{} before {}", w[0].0, w[1].0);
        }
        for (n, p) in ETEX_PRIMITIVES {
            assert_eq!(Prim::by_name_etex(n), Some(p));
            assert_eq!(p.name(), n);
            assert_eq!(Prim::by_name(n), None, "{n} is TeX82's");
        }
    }
}
