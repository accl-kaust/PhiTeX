//! What a node may define, read from its text by the syntactic pass
//! (`phitex-source`): the engine then makes readers of other fields wait
//! on nothing of it.
//!
//! A node's text is a slice (`slices_of`): a document's paragraph
//! region or loading command, or a whole format or package text, whose
//! hint is `Any`. The pass bounds the control sequences a slice may
//! define (`cs:` and `act:` fields: its
//! own statement and the bodies of the macros it calls, when those are
//! visible in the same text); every other field the run may set (the
//! lists, the page, fonts, conditionals: the engine's state) is covered
//! by prefixes, so the answer is [`MayDefine::Prefixes`] of the state
//! heads and the exact names, or [`MayDefine::Any`] where the pass
//! cannot bound it (a macro it has not seen, `\input`, a computed name).
//!
//! The pass's answer holds under its assumptions: the catcodes of the
//! bytes it read, and that the definer and the macros whose bodies it
//! read still have those meanings. [`verify`] checks them when the node
//! runs; a difference is a diagnostic ("hint regime violated"), and a
//! definition outside the bound is one too ("undeclared definition"),
//! both counted in [`crate::Stats`]. Neither changes the output: the
//! engine re-binds uses on every definition it sees, hints only decide
//! what waits.
//!
//! Where the text comes from: a document's or prelude's slices are
//! analysed whole in [`text_hints`] (the regime tracked across the text:
//! `\makeatletter`, `\ExplSyntaxOn`, literal `\catcode`s) and each slice's
//! hint goes into its node's arguments ([`hint_arg`]); a document file
//! read by `\input` is sliced and analysed the same way
//! (`Run::doc_file_regions`), its regions finding the analysis by name.
//! A slice that opens and closes no scope (`no_scope`) declares no
//! scope effect. Expansion tokens carry no text position and answer
//! `Any`.
//!
//! The pass reads bodies from the text itself and from the format's
//! definitions ([`Defs`]: the prelude's text and every file it read,
//! harvested once per prelude by `api.rs`): a call of a format macro is
//! bounded by its body like a call of the document's own, and a macro
//! whose body ends a paragraph marks a paragraph region's end wherever
//! it is called (OUTPUT.md §5).
//!
//! `PHITEX_HINTS=1` prints each distinct node text's answer and cause;
//! `PHITEX_HINTS_OFF=1` answers `Any` for every node (the comparison).

use crate::exec::LockWait as _;
use phitex_defuse::FxHashMap;
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use phitex_defuse::{Field, MayDefine, Value, ValueId, Values};
use phitex_source::pass::{self, Atom, AtomKind, Def, DefSet, Definitions, FileKind, Pass, Scope};

use crate::run::{R, Run};
use crate::val::{Mean, Tok, int_of, list_of, str_of};

/// The definitions the hints may bind calls to besides the text's own:
/// the format's (shared by every text of a build).
pub(crate) type Defs = Arc<pass::Definitions>;

/// The strings `parts` one after another (the pass builds its texts
/// without formatting, spec-final §5.3).
fn cat(parts: &[&str]) -> String {
    let mut s = String::with_capacity(parts.iter().map(|p| p.len()).sum());
    for p in parts {
        s.push_str(p);
    }
    s
}

/// No definitions (INITEX, a text analysed on its own).
pub(crate) fn no_defs() -> &'static Defs {
    static D: OnceLock<Defs> = OnceLock::new();
    D.get_or_init(|| Arc::new(pass::Definitions::new()))
}

/// What a node's text says it may define.
#[derive(Clone, Debug, Default)]
pub(crate) struct Hint {
    /// The bound, or `None`: anything.
    pub bound: Option<Bound>,
    /// Why it is unbounded, or how it was bounded.
    pub cause: String,
    /// The text, shortened.
    pub excerpt: String,
    /// For an unbounded hint: what its commands' rows and its calls'
    /// bodies define as far as the pass sees them (the engine's
    /// per-field question, [`Hint::may_define_field`]; a wrong `false`
    /// is refuted by the engine, never wrong output).
    pub guess: Option<Bound>,
}

/// A bounded definition set and what it rests on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Bound {
    /// The engine's fields (`cs:name`, `act:code`).
    pub names: Vec<String>,
    /// Prefixes of fields (`cs:name` for `\csname name…`).
    pub prefixes: Vec<String>,
    /// Bytes and the catcode each was read with.
    pub cats: Cats,
    /// Meaning fields (`cs:name`, `act:code`) and the class assumed: 0
    /// a primitive, 1 anything defined.
    pub means: Vec<(String, u8)>,
    /// The state fields (prefixes) the command's class may set, or
    /// `None`: every state head ([`state_prefixes`]).
    pub state: Option<Vec<&'static str>>,
    /// The command opens and closes no scope (its effect is `None`).
    pub no_scope: bool,
    /// For a slice: the following siblings its run takes, when the text
    /// says so exactly (`Some(0)`: none); `None` when only the run knows.
    pub claim: Option<usize>,
    /// The fields the node reads from outside its run, when known (a
    /// package load's record, [`crate::pkgrec`]): declared reads.
    pub reads: Vec<String>,
}

/// The catcode a [`Cats`] pair gives a byte read as no letter (a
/// control word's end, a control symbol's character): any but 11.
pub(crate) const NOT_LETTER: u8 = 16;

/// Bytes and the catcode each was read with, in the order first met,
/// each pair once: the list, and by byte the catcode it came with
/// first, so adding is mostly a compare, not a search.
#[derive(Clone, Debug)]
pub(crate) struct Cats {
    list: Vec<(u8, u8)>,
    /// By byte below 128, the catcode it was first added with (16:
    /// none); a byte added again with another catcode is searched for.
    first: [u8; 128],
}

impl Default for Cats {
    fn default() -> Cats {
        Cats {
            list: Vec::new(),
            first: [16; 128],
        }
    }
}

impl PartialEq for Cats {
    fn eq(&self, o: &Cats) -> bool {
        self.list == o.list
    }
}

impl Eq for Cats {}

impl std::ops::Deref for Cats {
    type Target = [(u8, u8)];
    fn deref(&self) -> &[(u8, u8)] {
        &self.list
    }
}

impl FromIterator<(u8, u8)> for Cats {
    fn from_iter<I: IntoIterator<Item = (u8, u8)>>(it: I) -> Cats {
        let mut c = Cats::default();
        for p in it {
            c.add(p);
        }
        c
    }
}

impl Cats {
    /// Adds the pair unless it is there (a byte below 128, a catcode
    /// below 16; any other pair is searched for).
    pub(crate) fn add(&mut self, (b, k): (u8, u8)) {
        if b < 128 && k < 16 {
            let f = &mut self.first[usize::from(b)];
            if *f == k {
                return;
            }
            if *f == 16 {
                *f = k;
                self.list.push((b, k));
                return;
            }
        }
        if !self.list.contains(&(b, k)) {
            self.list.push((b, k));
        }
    }

    pub(crate) fn clear(&mut self) {
        self.list.clear();
        self.first = [16; 128];
    }
}

/// The state fields (heads: a family's prefix with its colon, or a
/// cell's name) a command of this name may set besides control
/// sequences, from tex.web; `None`: a command whose state is not
/// listed here (every head). The alignment state (`align:`), `\\prevgraf`
/// (`pg:`) and `aftertoken` come from the context, not from the command ([`Analysis::cs_hint`]).
fn class_state(name: &str) -> Option<&'static [&'static str]> {
    // (one table lookup per name seen on the thread: the rows are a
    // long match of strings, asked for every call of every body)
    thread_local! {
        static C: std::cell::RefCell<FxHashMap<Box<str>, Option<&'static [&'static str]>>> =
            std::cell::RefCell::default();
    }
    if let Some(r) = C.with(|c| c.borrow().get(name).copied()) {
        return r;
    }
    let r = class_state_of(name);
    C.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= 1 << 16 {
            c.clear();
        }
        c.insert(Box::from(name), r);
    });
    r
}

/// The meaning field of the control sequence the pass names `name`
/// (`cs:` and its name, a `^^` form read), made once per name on the
/// thread: a body's relied names are asked for at every call.
fn cs_field(name: &str) -> Option<String> {
    thread_local! {
        static C: std::cell::RefCell<FxHashMap<Box<str>, Option<String>>> =
            std::cell::RefCell::default();
    }
    if let Some(r) = C.with(|c| c.borrow().get(name).cloned()) {
        return r;
    }
    let r = Tok::cs(&caret_name(name)).field_name();
    C.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= 1 << 16 {
            c.clear();
        }
        c.insert(Box::from(name), r.clone());
    });
    r
}

/// [`class_state`], looked up in its table.
#[allow(clippy::too_many_lines, clippy::match_same_arms)]
fn class_state_of(name: &str) -> Option<&'static [&'static str]> {
    let n = name
        .strip_prefix("tex_")
        .and_then(|x| x.strip_suffix(":D"))
        .unwrap_or(name);
    Some(match n {
        // definitions and contributions: the control sequence only, or
        // the list accumulators (§1218, §1221, §1224, §1103)
        "def" | "gdef" | "let" | "futurelet" | "chardef" | "mathchardef" | "countdef"
        | "dimendef" | "skipdef" | "muskipdef" | "toksdef" | "relax" | "global" | "long"
        | "outer" | "protected" | "the" | "number" | "romannumeral" | "string" | "meaning"
        | "show" | "showbox" | "showthe" | "showlists" | "showgroups" | "showtokens"
        | "showifs" | "dump" | "jobname" | "fontname" | "detokenize" | "unexpanded"
        | "ignorespaces" | "endcsname" | "prevdepth" | "spacefactor" | "nullfont"
        | "pagediscards" | "splitdiscards" | "noboundary" | "lastpenalty" | "lastskip"
        | "lastkern" | "lastnodetype" | "inputlineno" | "badness" | "eTeXversion"
        | "eTeXrevision" | "currentgrouplevel" | "currentgrouptype" | "currentiflevel"
        | "currentiftype" | "currentifbranch" | "numexpr" | "dimexpr" | "glueexpr" | "muexpr"
        | "gluestretch" | "glueshrink" | "gluestretchorder" | "glueshrinkorder" | "mutoglue"
        | "gluetomu" | "fontcharwd" | "fontcharht" | "fontchardp" | "fontcharic"
        | "parshapelength" | "parshapeindent" | "parshapedimen" | "topmark" | "firstmark"
        | "botmark" | "splitfirstmark" | "splitbotmark" | "topmarks" | "firstmarks"
        | "botmarks" | "splitfirstmarks" | "splitbotmarks" | "/" | " " | "mskip" | "mkern"
        | "penalty" | "end" | "beginL" | "endL" | "beginR" | "endR" => &[],
        // a paragraph's end (§1094-§1096): `\\prevgraf` (`pg:`, §1096);
        // the paragraph, the conditionals and the groups are run-local
        "par" => PAR_END,
        // a box's dimension (§1247): the box and its level fix
        "wd" | "ht" | "dp" => &["box:", "lvlfix"],
        // horizontal material: may start a paragraph ([`starts_par`] adds
        // `pg:`), the box level fix (§1090, §1091)
        // (`\\accent` does the assignments before its character: a font,
        // registers, §1123)
        // (the assignments between it and its character, §1123, are
        // commands of the text or of the calling body: their own rows)
        // (the only register material sets is `\\box255`, when the page
        // builder runs inline and fires the routine, §1091, §1094, §1012;
        // `\\unhbox`, `\\box` and `\\lastbox` void their own, §1110, §1079)
        "accent" => &["box:255", "lvlfix"],
        "char" | "discretionary" | "-" | "hskip" | "hfil" | "hfill" | "hss" | "hfilneg"
        | "vrule" | "indent" | "noindent" | "unhcopy" | "copy" | "leaders" | "cleaders"
        | "xleaders" => &["box:255", "lvlfix"],
        "unhbox" | "box" => &["box:", "lvlfix"],
        // vertical material and removals (§1056, §1105, §1080)
        "vskip" | "vfil" | "vfill" | "vss" | "vfilneg" | "kern" | "hrule" | "unvcopy"
        | "unskip" | "unpenalty" | "unkern" => &["box:255"],
        "unvbox" | "lastbox" => &["box:"],
        // math only (§1136-§1194): the incompleat noad is the run's own
        "mathord" | "mathop" | "mathbin" | "mathrel" | "mathopen" | "mathclose" | "mathpunct"
        | "mathinner" | "underline" | "overline" | "mathaccent" | "radical" | "displaystyle"
        | "textstyle" | "scriptstyle" | "scriptscriptstyle" | "left" | "right" | "middle"
        | "mathchar" | "delimiter" | "displaylimits" | "limits" | "nolimits" | "nonscript"
        | "mathchoice" | "eqno" | "leqno" | "over" | "atop" | "above" | "overwithdelims"
        | "atopwithdelims" | "abovewithdelims" => &[],
        // opening a group: a level of the run's own nest (§1063)
        "begingroup" | "bgroup" => &[],
        // closing a group: the box's badness, its level fix, inserts (§1100, §1079)
        // and the box a `\\setbox` or box context gets (§1077 box_end)
        // (an output routine's `}` resumes the page builder, §1100)
        // (a box's `}` sets a register only in a `\\setbox`'s context,
        // §1075, and that command's row names it; a `}` closing a box
        // begun before the node is a fragment's, [`fragment_context`])
        "endgroup" | "egroup" => &["badness", "lvlfix", "inspen"],
        // arguments expanded while read: a conditional may end (§366, §498)
        "edef" | "xdef" | "csname" | "message" | "errmessage" | "special" => &["cs:"],
        "mark" => &[],
        // (e-TeX's mark classes, §1494 ff.)
        "marks" => &["markclasses"],
        "write" | "openout" | "closeout" => &["wopen:", "write:"],
        // (`\\immediate` of a pdfTeX object too, pdfTeX §1528)
        "immediate" => &["wopen:", "write:", "pdf:"],
        // (§1099: `normal_paragraph` runs in the insert's own group, its
        // resets undone at the group's end, §283, §1100)
        "insert" | "vadjust" => &[],
        // (§638 `dead_cycles:=0`; §1374 the page's `\\write`, `\\openout`,
        // `\\closeout` whatsits, expanded, §1370; pdfTeX's at shipout)
        "shipout" => &["deadcycles", "write:", "wopen:", "pdf:"],
        // (the token parameter, §1226)
        "output" => &["param:output"],
        "afterassignment" => &["aftertoken"],
        "aftergroup" => &[],
        // boxes (§1083 begin_box, §1077 \\setbox); a box whose braces close in
        // the same node is packaged there and sets \\\\badness, §1086, §649;
        // no mark class: only `\\marks` makes one, its own row)
        "hbox" | "vbox" | "vtop" | "vcenter" => &["lvlfix", "badness"],
        "setbox" | "vsplit" => &["box:", "lvlfix", "badness"],
        // conditionals (§498-§510)
        "if" | "ifx" | "ifnum" | "ifdim" | "ifodd" | "ifvmode" | "ifhmode" | "ifmmode"
        | "ifinner" | "ifvoid" | "ifhbox" | "ifvbox" | "ifcat" | "ifcase" | "iftrue"
        | "iffalse" | "ifdefined" | "ifcsname" | "iffontchar" | "unless" | "else" | "fi" | "or"
        | "ifpdfprimitive" | "ifpdfabsnum" | "ifpdfabsdim" | "ifincsname" | "ifeof" => &[],
        // fonts (§1257 new_font, §1234)
        "font" => &[
            "fd:",
            "fdn:",
            "fontlast",
            "fontid:",
            "fontinfo:",
            "hc:",
            "sc:",
        ],
        "textfont" | "scriptfont" | "scriptscriptfont" => &["fam:"],
        // a font's parameters (§1253)
        "fontdimen" => &["fd:", "fdn:"],
        "hyphenchar" => &["hc:"],
        "skewchar" => &["sc:"],
        // codes (§1232)
        // (the code's value is scanned with expansion: a conditional
        // may end, §1232, §498)
        "catcode" => &["cat:"],
        "lccode" => &["lc:"],
        "uccode" => &["uc:"],
        "sfcode" => &["sf:"],
        "mathcode" => &["mc:"],
        "delcode" => &["dc:"],
        // registers (§1224-§1238): the register families, the
        // parameters (`\\advance\\tolerance`), a conditional expanded
        // in the value (§498), `\\afterassignment`'s token (§1269)
        // (a register by number assigns that family's register, §1224)
        "count" => &["count:", "aftertoken"],
        "dimen" => &["dimen:", "aftertoken"],
        "skip" => &["skip:", "aftertoken"],
        "muskip" => &["muskip:", "aftertoken"],
        "toks" => &["toks:", "aftertoken"],
        "advance" | "multiply" | "divide" => &[
            "count:",
            "dimen:",
            "skip:",
            "muskip:",
            "toks:",
            "param:",
            "aftertoken",
        ],
        "parshape"
        | "interlinepenalties"
        | "clubpenalties"
        | "widowpenalties"
        | "displaywidowpenalties" => &[
            "count:", "dimen:", "skip:", "muskip:", "toks:", "param:", "parshape",
        ],
        "prevgraf" => &["pg:"],
        "deadcycles" => &["deadcycles"],
        "insertpenalties" => &["inspen"],
        "pagegoal" | "pagetotal" | "pagestretch" | "pagefilstretch" | "pagefillstretch"
        | "pagefilllstretch" | "pageshrink" | "pagedepth" => &[],
        "batchmode" | "nonstopmode" | "scrollmode" | "errorstopmode" | "interactionmode" => {
            &["interaction"]
        }
        // (§960, §934: the pattern trie and exceptions)
        "hyphenation" | "patterns" => &["patterns"],
        // expansion only (§367, §369)
        "noexpand" => &[],
        // boxes in a box context (§1073 begin_box via §1084)
        "moveleft" | "moveright" | "raise" | "lower" => &["box:", "lvlfix", "badness"],
        // a language whatsit (§1377) in horizontal material, which may
        // start a paragraph (`pg:`, §1091)
        "setlanguage" => &["box:", "lvlfix", "pg:"],
        // (they may read or expand anything: every head, §366 \\expandafter,
        // §362 \\input/\\endinput, §1285 \\lowercase/\\uppercase)
        "expandafter" | "input" | "endinput" | "lowercase" | "uppercase" => return None,
        // alignments (§774-§800)
        "halign" | "valign" | "cr" | "crcr" | "noalign" | "omit" | "span" => &["align", "badness"],
        "read" | "readline" | "openin" | "closein" => &["read:"],
        // pdfTeX's objects, streams, random numbers, colour stacks
        _ if n.starts_with("pdf") && is_prim(n) => &["pdf:"],
        _ => {
            if is_prim(n) || table_param(n) {
                // (a parameter by name, or a primitive not listed: its
                // own parameter, or any head)
                if table_param(n) {
                    return param_row(n);
                }
                return None;
            }
            return None;
        }
    })
}

/// What `normal_paragraph` resets (§1070, e-TeX's
/// `\\interlinepenalties`).
/// What a paragraph's end may set (`\\par`, a blank line): see
/// [`class_state`]'s `par` row (§1070, §1094-§1096, §1199).
/// `normal_paragraph`'s resets (§1070) are not here: a reset that
/// writes the value already in force is no definition, and a value
/// differs from its reset only after a command set it since the last
/// paragraph end, a command whose own row declares it (`\\parshape`,
/// `\\hangindent`, ...), as `\\looseness` and `\\hangindent` were
/// already. Declaring `parshape` here parked every following
/// paragraph's `line_break` on its predecessor.
const PAR_END: &[&str] = &["pg:"];

const NORMAL_PARAGRAPH: &[&str] = &[
    "param:looseness",
    "param:hangindent",
    "param:hangafter",
    "parshape",
    "param:interlinepenalties",
];

/// What a math shift may set: math's list and `\\fam` (§1138, §1139),
/// a display's sizes and the lines of the paragraph before it (§1145,
/// §1146), and its end's resumption (§1200).
const MATH_SHIFT: &[&str] = &[
    "pg:",
    "param:fam",
    "param:predisplaysize",
    "param:displaywidth",
    "param:displayindent",
];

/// A parameter's own field (`param:<name>`), for a parameter assigned
/// by name (§1228).
fn param_row(name: &str) -> Option<&'static [&'static str]> {
    static ROWS: OnceLock<FxHashMap<&'static str, [&'static str; 1]>> = OnceLock::new();
    ROWS.get_or_init(|| {
        crate::prims::PRIMS
            .iter()
            .filter(|p| matches!(p.1, 72..=76))
            .map(|p| {
                let f: &'static str = Box::leak(cat(&["param:", p.0]).into_boxed_str());
                (p.0, [f])
            })
            .collect()
    })
    .get(name)
    .map(|r| &r[..])
}

/// Whether `name` is a primitive (a set built once).
fn is_prim(name: &str) -> bool {
    static S: OnceLock<phitex_defuse::FxHashSet<&'static str>> = OnceLock::new();
    S.get_or_init(|| crate::prims::PRIMS.iter().map(|p| p.0).collect())
        .contains(name)
}

/// Whether `name` is an integer, dimen, glue or token parameter.
fn table_param(name: &str) -> bool {
    static S: OnceLock<phitex_defuse::FxHashSet<&'static str>> = OnceLock::new();
    S.get_or_init(|| {
        crate::prims::PRIMS
            .iter()
            .filter(|p| matches!(p.1, 72..=76))
            .map(|p| p.0)
            .collect()
    })
    .contains(name)
}

/// The engine's state head of a field the pass names (`count:12`,
/// `param:\tolerance`, `catcode:124`, `misc:pdflastobj`).
fn head_of_pass_field(f: &str) -> Option<&'static str> {
    let fam = f.split(':').next().unwrap_or("");
    Some(match fam {
        "count" => "count:",
        "dimen" => "dimen:",
        "skip" => "skip:",
        "muskip" => "muskip:",
        "toks" => "toks:",
        "box" => "box:",
        "catcode" => "cat:",
        "lccode" => "lc:",
        "uccode" => "uc:",
        "sfcode" => "sf:",
        "mathcode" => "mc:",
        "delcode" => "dc:",
        "textfont" | "scriptfont" | "scriptscriptfont" => "fam:",
        "param" => "param:",
        "misc" => "pdf:",
        _ => return None,
    })
}

/// Whether `f` is one register of a family named as the engine names
/// it (`count:0`, `box:255`): its head is the field itself.
fn register_by_number(f: &str) -> bool {
    f.split_once(':').is_some_and(|(fam, n)| {
        matches!(fam, "count" | "dimen" | "skip" | "muskip" | "toks" | "box")
            && !n.is_empty()
            && n.bytes().all(|b| b.is_ascii_digit())
    })
}

/// Adds heads to a state set (`None`: every head already).
fn add_heads(st: &mut Option<Vec<&'static str>>, heads: Option<&[&'static str]>) {
    match (st.as_mut(), heads) {
        (Some(v), Some(hs)) => {
            for &h in hs {
                if !v.contains(&h) {
                    v.push(h);
                }
            }
        }
        (Some(_), None) => *st = None,
        (None, _) => {}
    }
}

fn add_head(st: &mut Option<Vec<&'static str>>, h: &'static str) {
    if let Some(v) = st.as_mut()
        && !v.contains(&h)
    {
        v.push(h);
    }
}

/// [`add_head`] for a head made at run time (a parameter's field, a
/// decoded hint's): interned once ([`head`]).
fn add_head_dyn(st: &mut Option<Vec<&'static str>>, h: &str) {
    if let Some(v) = st.as_mut()
        && !v.contains(&h)
    {
        v.push(head(h));
    }
}

/// A state head as a static string: the heads are few (tex.web's
/// families and cells, the parameters' fields), so each distinct one
/// made at run time is leaked once and a bound holds no strings.
fn head(h: &str) -> &'static str {
    type Heads = std::sync::RwLock<phitex_defuse::FxHashSet<&'static str>>;
    static H: OnceLock<Heads> = OnceLock::new();
    let heads = H.get_or_init(Heads::default);
    // (read shared: the chunks of a document are analysed on the pool)
    if let Some(&x) = heads.read().unwrap_or_else(PoisonError::into_inner).get(h) {
        return x;
    }
    let mut set = heads.write().unwrap_or_else(PoisonError::into_inner);
    if let Some(&x) = set.get(h) {
        return x;
    }
    let x: &'static str = Box::leak(h.to_owned().into_boxed_str());
    set.insert(x);
    x
}

/// The heads of every field a run may set besides the control sequences:
/// each lowercase letter but `c` and `a`, and those two followed by any
/// letter but the `s` of `cs:` and the `c` of `act:`.
fn state_prefixes() -> &'static [String] {
    static P: OnceLock<Vec<String>> = OnceLock::new();
    P.get_or_init(|| {
        let mut v = Vec::new();
        for c in b'a'..=b'z' {
            if c != b'c' && c != b'a' {
                v.push((c as char).to_string());
            }
        }
        for c in (b'A'..=b'Z').chain(b'0'..=b'9').chain(*b"_@.-/:") {
            v.push((c as char).to_string());
        }
        for c in b'a'..=b'z' {
            if c != b's' {
                v.push(cat(&["c", char::from(c).encode_utf8(&mut [0; 4])]));
            }
            if c != b'c' {
                v.push(cat(&["a", char::from(c).encode_utf8(&mut [0; 4])]));
            }
        }
        v
    })
}

/// Whether `f` is a control sequence's or active character's meaning.
fn is_cs_field(f: &str) -> bool {
    f.starts_with("cs:") || f.starts_with("act:")
}

impl Hint {
    fn any(cause: impl Into<String>, excerpt: &str) -> Hint {
        Hint {
            bound: None,
            cause: cause.into(),
            excerpt: short(excerpt),
            guess: None,
        }
    }

    /// The engine's answer.
    pub(crate) fn may_define(&self) -> MayDefine {
        match &self.bound {
            None => MayDefine::Any,
            Some(b) => {
                let mut ps: Vec<String> = match &b.state {
                    Some(st) => st.iter().map(|&h| h.to_owned()).collect(),
                    None => state_prefixes().to_vec(),
                };
                ps.extend(b.names.iter().cloned());
                ps.extend(b.prefixes.iter().cloned());
                // the page state and the shipped pages: any piece with
                // items for the main list folds them at its end and
                // defines the next page state (`crate::pages`), whatever
                // its commands; a reader past a piece not declaring them
                // would read an older version and run again
                for f in crate::pages::PAGE_FIELDS {
                    if !ps.iter().any(|p| f.starts_with(p.as_str())) {
                        ps.push(f.to_owned());
                    }
                }
                // (a cell's definer defines its record too)
                crate::fields::add_record_prefixes(&mut ps);
                MayDefine::Prefixes(ps)
            }
        }
    }

    /// `A` or `B`, the cause, the excerpt, then the bound's parts;
    /// records by `\x1e`, items by `\x1f`, pairs by `\x1d`.
    pub(crate) fn encode(&self) -> String {
        let mut o = String::new();
        match &self.bound {
            None => {
                o.push('A');
                o.push('\x1e');
                o.push_str(&self.cause);
                o.push('\x1e');
                o.push_str(&self.excerpt);
                if let Some(g) = &self.guess {
                    encode_bound(g, &mut o);
                }
            }
            Some(b) => {
                o.push('B');
                o.push('\x1e');
                o.push_str(&self.cause);
                o.push('\x1e');
                o.push_str(&self.excerpt);
                encode_bound(b, &mut o);
            }
        }
        o
    }

    pub(crate) fn decode(s: &str) -> Hint {
        let mut it = s.split('\x1e');
        let tag = it.next().unwrap_or("");
        let cause = it.next().unwrap_or("").to_owned();
        let excerpt = it.next().unwrap_or("").to_owned();
        if tag != "B" {
            let rest: Vec<&str> = it.collect();
            return Hint {
                bound: None,
                cause: if cause.is_empty() {
                    "hint not decodable".to_owned()
                } else {
                    cause
                },
                excerpt,
                guess: (tag == "A" && !rest.is_empty()).then(|| decode_bound(rest.into_iter())),
            };
        }
        Hint {
            bound: Some(decode_bound(it)),
            cause,
            excerpt,
            guess: None,
        }
    }

    /// Whether the node may define `f`: its bound's answer, or for an
    /// unbounded node its guess (what its commands' rows and the bodies
    /// of its calls define); `true` without one.
    pub(crate) fn may_define_field(&self, f: &str) -> bool {
        if false {
            return true;
        }
        // (a record: where the node may define one of its cells; the
        // cells' names written over one buffer per thread, no string
        // made per call)
        if let Some(b) = crate::fields::RECORDS.iter().position(|&r| r == f) {
            thread_local! {
                static CELL: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
            }
            return CELL.with(|c| {
                let mut c = c.borrow_mut();
                (0..crate::fields::record_len(b)).any(|i| {
                    crate::fields::record_cell_name_into(b, i, &mut c);
                    self.may_define_field(&c)
                })
            });
        }
        let Some(b) = self.bound.as_ref().or(self.guess.as_ref()) else {
            return true;
        };
        if b.names.iter().any(|n| n == f) || b.prefixes.iter().any(|p| f.starts_with(p.as_str())) {
            return true;
        }
        if is_cs_field(f) {
            return false;
        }
        match &b.state {
            None => true,
            Some(st) => st.iter().any(|h| f.starts_with(*h)),
        }
    }
}

/// `x` in decimal, as `{x}` formats it.
fn push_u8(o: &mut String, x: u8) {
    if x >= 100 {
        o.push(char::from(b'0' + x / 100));
    }
    if x >= 10 {
        o.push(char::from(b'0' + x / 10 % 10));
    }
    o.push(char::from(b'0' + x % 10));
}

/// A bound's fields after a hint's excerpt (`\x1e` before each).
fn encode_bound(b: &Bound, o: &mut String) {
    o.push('\x1e');
    o.push_str(&b.names.join("\x1f"));
    o.push('\x1e');
    o.push_str(&b.prefixes.join("\x1f"));
    o.push('\x1e');
    // (written in place: no string per pair, no formatting)
    for (i, &(c, k)) in b.cats.iter().enumerate() {
        if i > 0 {
            o.push('\x1f');
        }
        push_u8(o, c);
        o.push('\x1d');
        push_u8(o, k);
    }
    o.push('\x1e');
    for (i, (n, k)) in b.means.iter().enumerate() {
        if i > 0 {
            o.push('\x1f');
        }
        o.push_str(n);
        o.push('\x1d');
        let _ = write!(o, "{k}");
    }
    o.push('\x1e');
    match &b.state {
        None => o.push('*'),
        Some(st) => o.push_str(&st.join("\x1f")),
    }
    o.push('\x1e');
    o.push(if b.no_scope { 'N' } else { 'P' });
    o.push('\x1e');
    if let Some(n) = b.claim {
        o.push_str(&n.to_string());
    }
    if !b.reads.is_empty() {
        o.push('\x1e');
        o.push_str(&b.reads.join("\x1f"));
    }
}

fn decode_bound<'s>(mut it: impl Iterator<Item = &'s str>) -> Bound {
    let items = |s: &str| -> Vec<String> {
        if s.is_empty() {
            Vec::new()
        } else {
            s.split('\x1f').map(str::to_owned).collect()
        }
    };
    let names = items(it.next().unwrap_or(""));
    let prefixes = items(it.next().unwrap_or(""));
    let cats = items(it.next().unwrap_or(""))
        .iter()
        .filter_map(|p| {
            let (a, b) = p.split_once('\x1d')?;
            Some((a.parse().ok()?, b.parse().ok()?))
        })
        .collect();
    let means = items(it.next().unwrap_or(""))
        .iter()
        .filter_map(|p| {
            let (a, b) = p.split_once('\x1d')?;
            Some((a.to_owned(), b.parse().ok()?))
        })
        .collect();
    let state = match it.next().unwrap_or("*") {
        "*" => None,
        st => Some(items(st).iter().map(|h| head(h)).collect()),
    };
    let no_scope = it.next() == Some("N");
    let claim = it.next().and_then(|c| c.parse().ok());
    let reads = items(it.next().unwrap_or(""));
    Bound {
        names,
        prefixes,
        cats,
        means,
        state,
        no_scope,
        claim,
        reads,
    }
}

fn short(s: &str) -> String {
    // (an excerpt is only for the traces that print it: none is made on
    // the analysis's hot path otherwise)
    if !excerpts() {
        return String::new();
    }
    let t: String = s.chars().take(40).collect();
    t.replace('\n', "\\n")
}

/// The text from byte `s`, at most 40 bytes, cut on a character
/// boundary (the text is UTF-8: a cut inside a character is a panic).
fn excerpt_at(text: &str, s: usize) -> &str {
    let s = s.min(text.len());
    &text[s..text.floor_char_boundary(s + 40)]
}

/// Whether a trace that prints hints' excerpts is on.
fn excerpts() -> bool {
    crate::exec::obs_flag!("PHITEX_HINTS")
        || crate::exec::obs_flag!("PHITEX_SLICES_TRACE")
        || crate::exec::obs_flag!("PHITEX_TEX_TRACE")
}

// ---------------------------------------------------------------------
// From the pass.

/// The pass's field names as the engine's: `cs:\name` is `cs:name`, an
/// active character `cs:c` is `act:code`; a field of another family is
/// the engine's state (covered by the prefixes).
fn engine_field(f: &str) -> Option<String> {
    let rest = f.strip_prefix("cs:")?;
    if let Some(name) = rest.strip_prefix('\\') {
        return Tok::cs(&caret_name(name)).field_name();
    }
    let mut cs = rest.chars();
    match (cs.next(), cs.next()) {
        (Some(c), None) => Tok::ch(13, u32::from(c)).field_name(),
        _ => Tok::cs(rest).field_name(),
    }
}

/// A control symbol's name written with `^^` (§355): `^^M` is the
/// character 13, `^^7f` the character 127.
fn caret_name(name: &str) -> String {
    let b = name.as_bytes();
    if b.len() >= 3 && b.starts_with(b"^^") {
        let hex = |c: u8| c.is_ascii_digit() || (b'a'..=b'f').contains(&c);
        if b.len() == 4 && hex(b[2]) && hex(b[3]) {
            if let Ok(c) = u8::from_str_radix(&name[2..4], 16) {
                return char::from(c).to_string();
            }
        } else if b.len() == 3 && b[2] < 128 {
            let c = if b[2] < 64 { b[2] + 64 } else { b[2] - 64 };
            return char::from(c).to_string();
        }
    }
    name.to_owned()
}

/// A prefix of the pass's as the engine's (`cs:\pre` → `cs:pre`).
fn engine_prefix(f: &str) -> Option<String> {
    let rest = f.strip_prefix("cs:")?;
    let name = rest.strip_prefix('\\').unwrap_or(rest);
    Some(cat(&["cs:", name]))
}

/// Names that read on by expansion before their command is known: the
/// command run here is computed.
const LOOKTHROUGH: &[&str] = &[
    "expandafter",
    "exp_after:wN",
    "noexpand",
    "exp_not:N",
    "exp_not:n",
    "lowercase",
    "input",
    "endinput",
    "tex_input:D",
    "tex_endinput:D",
    "uppercase",
    "scantokens",
];

/// Whether a command with hint `h` may set the field `name`.
fn may_set(h: &Hint, name: &str) -> bool {
    match &h.bound {
        None => true,
        Some(b) => match &b.state {
            None => true,
            Some(st) => st.iter().any(|x| name.starts_with(*x)),
        },
    }
}

/// Prefixes of a command (§1211): the command after them runs here.
fn is_prefix(name: &str) -> bool {
    matches!(
        name,
        "global"
            | "long"
            | "outer"
            | "protected"
            | "tex_global:D"
            | "tex_long:D"
            | "tex_outer:D"
            | "tex_protected:D"
    )
}

/// One text analysed whole: its slices (for the document's nodes), and
/// the hint of the command at each atom.
pub(crate) struct TextHints {
    pub slices: Vec<(usize, usize)>,
    /// Byte starts of the atoms a span reads (no space, no character
    /// after the first of its run), and the hint of the command
    /// beginning there.
    starts: Vec<u32>,
    /// (`None` for a skipped atom: never read)
    hints: Vec<Option<Arc<Hint>>>,
    /// Per atom: an operand an earlier command reads (never run on its
    /// own).
    skip: Vec<bool>,
    /// Per slice: whether a paragraph starts in it (a region: its run
    /// begins and ends in the vertical list).
    pars: Vec<bool>,
    /// Per slice.
    slice_hints: Vec<Arc<str>>,
    /// Per atom (parallel to `starts`): what its hint depends on in the
    /// pass's structure ([`atom_sig`]), for an analysis of the edited
    /// text that keeps the hints of the atoms the edit did not reach
    /// ([`Reuse`]).
    sigs: Vec<u64>,
}

/// A chunk's last analysis and the edit since (`(a, b, c)`: the old
/// bytes `a..b` are now `a..c`), for [`text_hints_reads_reusing`]: the
/// atoms outside the slices around the edit whose structure is the
/// same keep their hints.
pub(crate) struct Reuse<'a> {
    pub th: &'a TextHints,
    pub edit: (usize, usize, usize),
}

impl TextHints {
    /// The hint of the command at or after byte `off`.
    fn at(&self, off: usize) -> Hint {
        // (to the end of the slice holding `off`: the node's own text)
        let end = self
            .slices
            .iter()
            .find(|&&(s, e)| s <= off && off < e)
            .map_or(usize::MAX, |x| x.1);
        self.at_until(off, end)
    }

    /// The hint of the text from byte `off` to byte `end`: the union of
    /// the bounds of the commands in it (a run executes all of them: a
    /// line of a file read by `\input`, a region).
    pub(crate) fn at_until(&self, off: usize, end: usize) -> Hint {
        let k = self.starts.partition_point(|&s| (s as usize) < off);
        let mut acc: Option<Union> = None;
        let mut first: Option<usize> = None;
        // (the first unbounded command, and whether every unbounded one
        // has a guess: the span's guess is then the union)
        let mut any: Option<Hint> = None;
        let mut guessed = true;
        let mut seen: phitex_defuse::FxHashSet<usize> = phitex_defuse::FxHashSet::default();
        let mut a = k;
        while let Some(&s) = self.starts.get(a)
            && (s as usize) < end
        {
            a += 1;
            if self.skip[a - 1] {
                continue;
            }
            let Some(h) = self.hints[a - 1].as_deref() else {
                continue;
            };
            if first.is_none() {
                first = Some(a - 1);
            }
            let part = if let Some(b) = &h.bound {
                b
            } else {
                guessed = guessed && h.guess.is_some();
                if any.is_none() {
                    any = Some(h.clone());
                }
                let Some(g) = &h.guess else { continue };
                g
            };
            // (a bound already taken in adds nothing again: a union is
            // idempotent, and a run's hint is shared by its repeats)
            if !seen.insert(std::ptr::from_ref(part) as usize) {
                continue;
            }
            match &mut acc {
                Some(acc) => acc.add(part),
                None => acc = Some(Union::new(part.clone())),
            }
        }
        let acc = acc.map(|u| u.b);
        if let Some(mut h) = any {
            h.guess = if guessed { acc } else { None };
            return h;
        }
        match (first, acc) {
            (Some(i), Some(b)) => {
                let h = self.hints[i].as_deref().expect("an atom not skipped");
                Hint {
                    bound: Some(b),
                    cause: h.cause.clone(),
                    excerpt: h.excerpt.clone(),
                    guess: h.guess.clone(),
                }
            }
            (Some(i), None) => self.hints[i]
                .as_deref()
                .cloned()
                .expect("an atom not skipped"),
            // (a slice of blanks only: a space in vertical mode does
            // nothing, §1045)
            (None, _) if end != usize::MAX && off < end => Hint {
                bound: Some(Bound {
                    state: Some(Vec::new()),
                    no_scope: true,
                    ..Bound::default()
                }),
                cause: "blanks".to_owned(),
                excerpt: String::new(),
                guess: None,
            },
            (None, _) => Hint::any("end of text", ""),
        }
    }

    fn slice_hint(&self, start: usize) -> Option<&Arc<str>> {
        let k = self.slices.partition_point(|&(s, _)| s < start);
        (self.slices.get(k).map(|x| x.0) == Some(start)).then(|| &self.slice_hints[k])
    }
}

/// The slices of a text, each with the blanks before the next.
///
/// A format's or a package's text (`Initex`, `Package`) is one slice,
/// the whole file: one node, a sequential run of it (nested inputs
/// inline), its output the definitions in force at its end.
///
/// A document's slices are its paragraph regions (`docs/TARGET_GRAPH.md`):
/// a region runs from its start to the end of the next item the pass
/// marks paragraph-ending (`Implicit::EndsParagraph`: a blank line,
/// `\par`, a vertical command, a macro whose body ends in one) that is
/// at brace depth 0, consumed by no earlier item, and outside every
/// conditional. So a region is balanced: its run opens and closes its
/// own groups and conditionals and leaves nothing to another node.
/// A loading command (`\documentclass`, `\usepackage`, `\RequirePackage`,
/// `\LoadClass`, `\input`, `\include`) with its `[...]` and `{...}`
/// arguments is one slice of its own (vertical: a paragraph before it
/// ends there). Any other control sequence at depth 0 takes the
/// `[...]`/`{...}` groups right after it into its slice
/// (`\centerline{...}`).
/// The units of a text as the hint pass slices a document's text
/// ([`slices_of`] with no prelude definitions): byte ranges, in order.
/// The edit-locality sweep (`tests/sweep`) edits one word in each.
#[must_use]
pub fn paragraph_units(text: &str) -> Vec<(usize, usize)> {
    let kind = FileKind::Document;
    let an = Analysis::with_own(text, kind, no_defs().clone(), own_definitions(text, kind));
    slices_of(text, &an, kind).0
}

// (every per-atom flag here is kept as its ranges, `Spans`, and every
// loop walks the commands and braces only or jumps over the ranges: a
// paragraph's plain text is never visited atom by atom)
#[allow(clippy::too_many_lines)]
fn slices_of(text: &str, an: &Analysis<'_>, kind: FileKind) -> (Vec<(usize, usize)>, Vec<bool>) {
    if kind != FileKind::Document {
        return if text.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            (vec![(0, text.len())], vec![true])
        };
    }
    let p = &an.p;
    let n = p.atoms.len();
    // brace depth before each atom
    let depth = |k: usize| an.depth(k);
    // atoms an earlier item claims (operands: never a slice's start)
    let claimed = Spans::of(
        p.items
            .iter()
            .enumerate()
            .map(|(i, it)| {
                // (a value whose last token is a command the pass cannot
                // read through, `\count1=\input inc-num`: the text up to
                // the next command is the value's too, §440)
                let end = match &it.hints.claim {
                    pass::Claim::Pending { cause, .. }
                        if matches!(*cause, "command in value position" | "value is a macro") =>
                    {
                        p.items.get(i + 1).map_or(it.end, |x| x.atom).max(it.end)
                    }
                    _ => it.end,
                };
                (u(it.atom + 1), u(end.min(n)))
            })
            .collect(),
    );
    // (that extension runs in the command's own run: `\\vskip 1in
    // \\centerline{…}` expands the macro while it looks for `plus`,
    // §1094, and runs what it leaves; one slice)
    let mut extended = Vec::new();
    for (i, it) in p.items.iter().enumerate() {
        if let pass::Claim::Pending { cause, .. } = &it.hints.claim
            && matches!(*cause, "command in value position" | "value is a macro")
        {
            // (to the next command; a loader the scan expands, `\\font\\x
            // =cmr12 \\input part`, to the end of its file name: the
            // text after it is a slice of its own)
            let end = match p.items.get(i + 1) {
                Some(x) if matches!(x.name.as_str(), "input" | "include") => x.end,
                Some(x) => x.atom,
                None => it.end,
            }
            .max(it.end);
            extended.push((u(it.end.max(it.atom + 1)), u(end.min(n))));
        }
    }
    // the operands of a primitive (`\sfcode`\)=0`): never a slice's
    // start, so a command and what it reads are one node (a macro's
    // operands are the pass's guess and do not count)
    let operand = Spans::of(
        p.items
            .iter()
            .filter(|it| is_prim(&it.name))
            .map(|it| (u(it.atom + 1), u(it.end.min(n))))
            .chain(extended)
            .collect(),
    );
    // the conditional nesting before each atom, by the text's items
    // (`\if…` +1, `\fi` -1; an item an earlier one consumes not counted):
    // the nesting after each change, by its atom
    let mut ifd_at: Vec<(usize, i64)> = Vec::new();
    {
        let mut consumed = 0;
        let mut cur = 0i64;
        for it in &p.items {
            if it.atom >= consumed {
                let d = if CONDITIONALS.contains(&it.name.as_str()) {
                    1
                } else if it.name == "fi" {
                    -1
                } else {
                    0
                };
                if d != 0 {
                    cur = (cur + d).max(0);
                    ifd_at.push((it.atom, cur));
                }
            }
            if it.consumes {
                consumed = consumed.max(it.end);
            }
        }
    }
    let ifd = |a: usize| {
        ifd_at
            .partition_point(|x| x.0 < a)
            .checked_sub(1)
            .map_or(0, |i| ifd_at[i].1)
    };
    // inside a brace group of the text opened at depth 0: the slice goes
    // on to its end (a brace group of the text is never split)
    let mut groups = Vec::new();
    for &k in &an.structural {
        let k = k as usize;
        let a = &p.atoms[k];
        // (a brace knows its mate's atom index; `p.braces` indexes tokens)
        if a.kind == AtomKind::Open && a.mate != usize::MAX && a.mate > k && depth(k) == 0 {
            groups.push((u(k + 1), u((a.mate + 1).min(n))));
        }
    }
    let groups = Spans::of(groups);
    // (a `\\begingroup`...`\\endgroup` pair or an environment is no
    // bar to a cut: a paragraph end inside it at brace depth 0 of the
    // text is one, the region that leaves the group open leaves it to the
    // engine as a scope and the one that closes it closes that scope,
    // `Run::run_all`, `Run::close_group`)
    // a control sequence at depth 0 and its `[...]`/`{...}` arguments
    // (with the spaces between): one slice; a loading command's are
    // never cut and end a paragraph before it
    let mut loaders: Vec<usize> = Vec::new();
    let mut commands = Spans::default();
    for &k in &an.structural {
        let k = k as usize;
        let a = &p.atoms[k];
        if a.kind != AtomKind::Cs
            || depth(k) != 0
            || ifd(k) != 0
            || groups.has(k)
            || commands.has(k)
            || claimed.has(k)
        {
            continue;
        }
        let name = a.name(text);
        // (no name is a loader: a command's run claims what it reads,
        // and the engine verifies the claim)
        let loader = false;
        let mut last = k;
        let mut j = k + 1;
        loop {
            while j < n && p.atoms[j].kind == AtomKind::Space {
                j += 1;
            }
            let Some(b) = p.atoms.get(j) else { break };
            if b.kind == AtomKind::Open && b.mate != usize::MAX && b.mate > j {
                last = b.mate;
                j = b.mate + 1;
            } else if b.kind == AtomKind::Char('[') {
                let mut q = j + 1;
                while q < n && !(p.atoms[q].kind == AtomKind::Char(']') && depth(q) == depth(j)) {
                    if p.atoms[q].kind == AtomKind::Par {
                        q = n;
                        break;
                    }
                    q += 1;
                }
                if q >= n {
                    break;
                }
                last = q;
                j = q + 1;
            } else if b.kind == AtomKind::Char('=')
                && last == k
                && claimed.has(j)
                && p.atoms
                    .get(j + 1)
                    .is_some_and(|x| x.kind == AtomKind::Open && x.mate != usize::MAX)
            {
                // (`\headline={...}`: a register the format defines, its
                // `=` and value read by it, one slice)
                last = p.atoms[j + 1].mate;
                j = last + 1;
            } else if loader && last == k && matches!(name, "input" | "include") {
                // `\input file`: the name runs to a space or a command
                let mut q = j;
                while q < n && matches!(p.atoms[q].kind, AtomKind::Char(_)) {
                    q += 1;
                }
                if q > j {
                    last = q - 1;
                }
                break;
            } else {
                break;
            }
        }
        if loader {
            loaders.push(k);
            // (a package load's slice takes the blanks after it, its
            // line's end: `\@ifnextchar[` skips them looking for an
            // optional argument, so a node of their own would be taken
            // by the load's run, `crate::pkgrec`)
            if !matches!(name, "input" | "include") {
                while p
                    .atoms
                    .get(last + 1)
                    .is_some_and(|x| x.kind == AtomKind::Space)
                {
                    last += 1;
                }
            }
        }
        commands.push(k + 1, (last + 1).min(n));
    }
    let inside = groups.union(&commands);
    // a paragraph: from its first horizontal atom at depth 0 (a
    // character, a command of §1090) to the end of the next item the
    // pass marks paragraph-ending (a blank line, `\par`, a vertical
    // command, a macro whose body ends in one); one slice, so its run
    // begins and ends in the vertical list
    let mut ends: Vec<usize> = Vec::new();
    {
        let mut consumed = 0;
        for it in &p.items {
            if it.atom >= consumed
                && it.atom < n
                && depth(it.atom) == 0
                && it.hints.implicit == pass::Implicit::EndsParagraph
                && ifd(it.atom) == 0
                // these read keywords or a number past their text, and
                // put back what follows (§1094): the paragraph goes on
                && !matches!(
                    it.name.as_str(),
                    "vskip" | "hrule" | "unvbox" | "unvcopy" | "halign"
                )
            {
                ends.push(it.end.max(it.atom + 1));
            }
            if it.consumes {
                consumed = consumed.max(it.end);
            }
        }
        for m in &p.marks {
            if m.atom < n
                && depth(m.atom) == 0
                && m.hints.implicit == pass::Implicit::EndsParagraph
                && ifd(m.atom) == 0
            {
                ends.push(m.atom + 1);
            }
        }
        // (a loading command is vertical: the paragraph ends before it)
        ends.extend(loaders.iter().copied());
        ends.sort_unstable();
        ends.dedup();
    }
    let mut par_at: Vec<usize> = Vec::new();
    let mut regions = Spans::default();
    {
        let mut k = 0;
        while k < n {
            // (an atom inside a range never starts a paragraph: the range
            // is passed whole)
            if let Some(e) = inside.end_of(k).or_else(|| regions.end_of(k)) {
                k = e;
                continue;
            }
            let a = &p.atoms[k];
            let horizontal = match a.kind {
                AtomKind::Char(_) | AtomKind::Verb => true,
                AtomKind::Cs => {
                    let name = a.name(text);
                    // (a macro, or a name undefined here, right before a
                    // horizontal atom: its run may start the paragraph,
                    // `\hello\ world.`, so the paragraph starts at it)
                    starts_par(name)
                        || (!is_prim(name) && !loaders.contains(&k) && {
                            let mut j = k + 1;
                            while p.atoms.get(j).is_some_and(|x| x.kind == AtomKind::Space) {
                                j += 1;
                            }
                            j < n
                                && !claimed.has(j)
                                && !inside.has(j)
                                && !regions.has(j)
                                && p.atoms.get(j).is_some_and(|x| match x.kind {
                                    AtomKind::Char(_) | AtomKind::Verb => true,
                                    AtomKind::Cs => starts_par(x.name(text)),
                                    _ => false,
                                })
                        })
                }
                _ => false,
            };
            // (an operand a macro is guessed to read still starts one: the
            // guess is not the run, `\hello\ world.`)
            if horizontal && depth(k) == 0 && ifd(k) == 0 && !operand.has(k) {
                // the first atom after the paragraph's end, at depth 0
                // and outside every conditional
                let mut e = ends.partition_point(|&x| x <= k);
                let mut end = ends.get(e).copied().unwrap_or(n);
                while end < n && (depth(end) != 0 || ifd(end) != 0) {
                    end += 1;
                    e += 1;
                    if let Some(&x) = ends.get(e) {
                        end = end.max(x);
                    }
                }
                par_at.push(k);
                regions.push(k + 1, end.min(n));
                k = end.max(k + 1);
                continue;
            }
            k += 1;
        }
    }
    let inside = inside.union(&regions);
    if crate::exec::obs_flag!("PHITEX_SLICES_TRACE") {
        for (k, a) in p.atoms.iter().enumerate() {
            eprintln!(
                "slices: {k} {:?} {:?} depth {} ifd {} inside {} claimed {} operand {} par {}",
                a.kind,
                &text[a.start..a.end],
                depth(k),
                ifd(k),
                inside.has(k),
                claimed.has(k),
                operand.has(k),
                par_at.binary_search(&k).is_ok()
            );
        }
        for it in &p.items {
            eprintln!(
                "slices: item {:?} {}..{} {:?}",
                it.name, it.atom, it.end, it.hints.implicit
            );
        }
    }
    let mut starts: Vec<usize> = vec![0];
    let mut prev_char = false;
    let mut k = 0;
    while k < n {
        // (no atom inside a range starts a slice: the range is passed
        // whole)
        if let Some(e) = inside.end_of(k) {
            prev_char = matches!(p.atoms[e - 1].kind, AtomKind::Char(_) | AtomKind::Verb);
            k = e;
            continue;
        }
        let a = &p.atoms[k];
        let ch = matches!(a.kind, AtomKind::Char(_) | AtomKind::Verb);
        let inside_if =
            ifd(k) > 0 && (a.kind != AtomKind::Cs || !CONDITIONALS.contains(&a.name(text)));
        // (a token after a catcode change the pass could not read, a
        // `\catcode` with a computed value, is tokenized by the run
        // alone, under the table as the commands before it on the line
        // left it (§343): the pass's reading of it, under the regime
        // before the change, proposes no cut there; the text is one
        // slice with the change until the pass reads for sure again)
        let pending = p
            .tokens
            .get(a.tok)
            .is_some_and(|t| t.tz == pass::Status::Pending);
        // (no slice begins right after a control sequence's name: where
        // a name ends is the pass's reading under the regime it predicts,
        // and a regime it got wrong, a letter read as another character,
        // would cut the name in two, which no run joins again, §356)
        let after_name = k > 0 && {
            let b = &p.atoms[k - 1];
            b.kind == AtomKind::Cs && b.end == a.start
        };
        // (an operand a primitive reads never starts a slice; a macro's
        // operand range is the pass's guess and does not cut)
        if !(inside_if || operand.has(k) || pending || (ch && prev_char) || after_name) {
            starts.push(a.start);
        }
        prev_char = ch;
        k += 1;
    }
    starts.sort_unstable();
    starts.dedup();
    let mut out = Vec::with_capacity(starts.len());
    let mut pars = Vec::with_capacity(starts.len());
    for (i, &s) in starts.iter().enumerate() {
        let e = starts.get(i + 1).copied().unwrap_or(text.len());
        if e > s {
            // (the slice holds a paragraph if one starts in it)
            // (or a horizontal atom at depth 0 outside conditionals that
            // only a macro's guessed operands cover: `\hello\ world.`)
            // (atoms are in text order: the slice's range only)
            let lo = p.atoms.partition_point(|a| a.start < s);
            let hi = lo + p.atoms[lo..].partition_point(|a| a.start < e);
            let has_par = par_at
                .get(par_at.partition_point(|&x| x < lo))
                .is_some_and(|&x| x < hi)
                || (lo..hi).any(|k| {
                    let a = &p.atoms[k];
                    depth(k) == 0
                        && ifd(k) == 0
                        && !operand.has(k)
                        && match a.kind {
                            AtomKind::Char(_) | AtomKind::Verb => true,
                            AtomKind::Cs => starts_par(a.name(text)),
                            _ => false,
                        }
                });
            out.push((s, e));
            pars.push(has_par);
        }
    }
    (out, pars)
}

/// Whether the atoms of `text[s..e]` open and close their groups
/// themselves: braces pair up and never close an outer one, so do
/// `\\begingroup`/`\\endgroup` and `\\bgroup`/`\\egroup`, and no macro
/// called (its closure, transitively) uses one of a pair without the
/// other.
fn balanced(an: &Analysis<'_>, text: &str, s: usize, e: usize) -> bool {
    let (mut brace, mut bg, mut sg) = (0i64, 0i64, 0i64);
    // (the union of the closures of the slice's calls: a pair may be
    // split between two macros, `\\fo@t`'s `\\bgroup` and the
    // `\\egroup` of the `\\@foot` it hands to `\\aftergroup`)
    let mut calls: Vec<String> = Vec::new();
    // (atoms are in text order: the slice's range only)
    // (only its commands and braces count)
    let lo = an.p.atoms.partition_point(|a| a.start < s);
    let hi = an.p.atoms.partition_point(|a| a.start < e);
    let st = &an.structural;
    let (c0, c1) = (
        st.partition_point(|&x| (x as usize) < lo),
        st.partition_point(|&x| (x as usize) < hi),
    );
    for &k in &st[c0..c1.max(c0)] {
        let a = &an.p.atoms[k as usize];
        match a.kind {
            AtomKind::Open => brace += 1,
            AtomKind::Close => brace -= 1,
            AtomKind::Cs => {
                let n = a.name(text);
                match n {
                    "begingroup" => sg += 1,
                    "endgroup" => sg -= 1,
                    "bgroup" => bg += 1,
                    "egroup" => bg -= 1,
                    _ if is_prim(n) => {}
                    _ => {
                        if crate::exec::obs_flag!("PHITEX_SLICES_TRACE") {
                            let set = an.defset(n);
                            let g: Vec<&str> = ["begingroup", "endgroup", "bgroup", "egroup"]
                                .into_iter()
                                .filter(|x| set.calls.iter().any(|c| c == x))
                                .collect();
                            if !g.is_empty() || !set.unbounded.is_empty() {
                                eprintln!(
                                    "slices: balance \\{n}: {g:?} unbounded {:?} calls {}",
                                    set.unbounded.first(),
                                    set.calls.len()
                                );
                            }
                        }
                        for c in &an.defset(n).calls {
                            if !calls.contains(c) {
                                calls.push(c.clone());
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        if brace < 0 || bg < 0 || sg < 0 {
            return false;
        }
    }
    let has = |x: &str| calls.iter().any(|c| c == x);
    brace == 0
        && bg == 0
        && sg == 0
        && has("begingroup") == has("endgroup")
        && has("bgroup") == has("egroup")
}

/// The commands that scan on past their text for a keyword or a unit
/// (`plus`, `minus`, `width`, `true`), expanding what follows.
const SCANS_ON: &[&str] = &[
    "font", "vskip", "hskip", "mskip", "hrule", "vrule", "kern", "mkern",
];

/// The name of the last item starting in the slice.
fn last_item(p: &Pass, text: &str, (s, e): (usize, usize)) -> Option<String> {
    let _ = text;
    // (items and atoms are in text order: the slice's range only)
    let lo = p.atoms.partition_point(|a| a.start < s);
    let hi = p.atoms.partition_point(|a| a.start < e);
    let k = p.items.partition_point(|it| it.atom < hi);
    k.checked_sub(1)
        .map(|k| &p.items[k])
        .filter(|it| it.atom >= lo)
        .map(|it| it.name.clone())
}

/// The first control sequence atom of the slice, if it starts with one.
fn first_cs(p: &Pass, text: &str, (s, e): (usize, usize)) -> Option<String> {
    let lo = p.atoms.partition_point(|a| a.start < s);
    let a = p.atoms[lo..]
        .iter()
        .take_while(|a| a.start < e)
        .find(|a| a.kind != AtomKind::Space)?;
    (a.kind == AtomKind::Cs).then(|| a.name(text).to_owned())
}

/// The primitive conditionals (§487) and e-TeX's and pdfTeX's.
const CONDITIONALS: &[&str] = &[
    "if",
    "ifcat",
    "ifnum",
    "ifdim",
    "ifodd",
    "ifvmode",
    "ifhmode",
    "ifmmode",
    "ifinner",
    "ifvoid",
    "ifhbox",
    "ifvbox",
    "ifx",
    "ifeof",
    "iftrue",
    "iffalse",
    "ifcase",
    "ifdefined",
    "ifcsname",
    "iffontchar",
    "ifincsname",
    "ifpdfabsnum",
    "ifpdfabsdim",
    "ifpdfprimitive",
];

/// The end of the span a file-rest node runs from byte `off`: its line,
/// and the lines after it while a brace group or a conditional opened
/// in the span is still open (the run reads on until it is closed,
/// `Run::run_all`).
fn line_span_end(text: &str, off: usize) -> usize {
    let bytes = text.as_bytes();
    let mut i = off.min(bytes.len());
    let (mut depth, mut ifs) = (0i64, 0i64);
    loop {
        let line_end = bytes[i..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(bytes.len(), |k| i + k);
        let line = &text[i..line_end];
        let mut cs = false;
        let mut name = String::new();
        let mut rest = line.chars().peekable();
        while let Some(c) = rest.next() {
            if c == '%' && !cs {
                break;
            }
            if cs {
                if c.is_ascii_alphabetic() {
                    name.push(c);
                    continue;
                }
                cs = false;
                match name.as_str() {
                    n if CONDITIONALS.contains(&n) => ifs += 1,
                    "fi" => ifs = (ifs - 1).max(0),
                    "begingroup" => depth += 1,
                    "endgroup" => depth = (depth - 1).max(0),
                    _ => {}
                }
                name.clear();
                if c == '\\' {
                    // (`\\` then a symbol: the symbol is the name)
                    cs = true;
                    if let Some(&d) = rest.peek()
                        && !d.is_ascii_alphabetic()
                    {
                        rest.next();
                        cs = false;
                    }
                    continue;
                }
            }
            match c {
                '\\' => {
                    cs = true;
                    if let Some(&d) = rest.peek()
                        && !d.is_ascii_alphabetic()
                    {
                        // a control symbol (`\{`, `\}`, `\%`)
                        rest.next();
                        cs = false;
                    }
                }
                '{' => depth += 1,
                '}' => depth = (depth - 1).max(0),
                _ => {}
            }
        }
        if cs {
            match name.as_str() {
                n if CONDITIONALS.contains(&n) => ifs += 1,
                "fi" => ifs = (ifs - 1).max(0),
                "begingroup" => depth += 1,
                "endgroup" => depth = (depth - 1).max(0),
                _ => {}
            }
        }
        if (depth == 0 && ifs == 0) || line_end >= bytes.len() {
            return line_end;
        }
        i = line_end + 1;
    }
}

/// The union of bounds: what a slice of several commands may define.
/// `None` state in either is `None` (every head).
/// A union of bounds made one by one ([`union`]'s result), with sets of
/// what it holds kept as it grows: no set of it made again per bound
/// added (a span's bound is the union of its many commands').
struct Union {
    b: Bound,
    prefixes: phitex_defuse::FxHashSet<String>,
    means: phitex_defuse::FxHashSet<String>,
    names: phitex_defuse::FxHashSet<String>,
}

impl Union {
    fn new(b: Bound) -> Union {
        Union {
            prefixes: b.prefixes.iter().cloned().collect(),
            means: b.means.iter().map(|m| m.0.clone()).collect(),
            names: b.names.iter().cloned().collect(),
            b,
        }
    }

    /// [`union`] of the bound so far and `o`.
    fn add(&mut self, o: &Bound) {
        for p in &o.prefixes {
            if self.prefixes.insert(p.clone()) {
                self.b.prefixes.push(p.clone());
            }
        }
        for &c in o.cats.iter() {
            self.b.cats.add(c);
        }
        // (a meaning an earlier command of the span defines is the run's
        // own, not assumed of the state before it)
        for m in &o.means {
            if !self.names.contains(&m.0) && self.means.insert(m.0.clone()) {
                self.b.means.push(m.clone());
            }
        }
        for n in &o.names {
            if self.names.insert(n.clone()) {
                self.b.names.push(n.clone());
            }
        }
        match (&mut self.b.state, &o.state) {
            (Some(sa), Some(sb)) => {
                for h in sb {
                    if !sa.contains(h) {
                        sa.push(*h);
                    }
                }
            }
            (st, None) => *st = None,
            (None, Some(_)) => {}
        }
        self.b.no_scope = self.b.no_scope && o.no_scope;
        self.b.claim = None;
    }
}

fn union(a: &mut Bound, b: &Bound) {
    use phitex_defuse::FxHashSet;
    // (each list kept in its order, each entry once: what `a` holds is
    // looked up in a set made once, not searched per entry of `b`)
    let prefixes: Vec<String> = {
        let mut have: FxHashSet<&str> = a.prefixes.iter().map(String::as_str).collect();
        b.prefixes
            .iter()
            .filter(|p| have.insert(p.as_str()))
            .cloned()
            .collect()
    };
    a.prefixes.extend(prefixes);
    for &c in b.cats.iter() {
        a.cats.add(c);
    }
    // (a meaning an earlier command of the span defines is the run's
    // own, not assumed of the state before it)
    let means: Vec<(String, u8)> = {
        let names: FxHashSet<&str> = a.names.iter().map(String::as_str).collect();
        let mut have: FxHashSet<&str> = a.means.iter().map(|x| x.0.as_str()).collect();
        b.means
            .iter()
            .filter(|m| !names.contains(m.0.as_str()) && have.insert(m.0.as_str()))
            .cloned()
            .collect()
    };
    a.means.extend(means);
    let names: Vec<String> = {
        let mut have: FxHashSet<&str> = a.names.iter().map(String::as_str).collect();
        b.names
            .iter()
            .filter(|n| have.insert(n.as_str()))
            .cloned()
            .collect()
    };
    a.names.extend(names);
    match (&mut a.state, &b.state) {
        (Some(sa), Some(sb)) => {
            for h in sb {
                if !sa.contains(h) {
                    sa.push(*h);
                }
            }
        }
        (st, None) => *st = None,
        (None, Some(_)) => {}
    }
    a.no_scope = a.no_scope && b.no_scope;
    a.claim = None;
}

/// The analysis of one text under `kind`'s regime.
/// An atom index as stored (a text's atoms number under 2^32).
fn u(k: usize) -> u32 {
    u32::try_from(k).unwrap_or(u32::MAX)
}

/// A set of atoms kept as its ranges `[s, e)`, sorted and disjoint: a
/// flag of the atoms with no vector as long as the text (plain text is
/// one range, or none).
#[derive(Default)]
struct Spans(Vec<(u32, u32)>);

impl Spans {
    /// The union of `ranges`, in any order.
    fn of(mut ranges: Vec<(u32, u32)>) -> Spans {
        ranges.retain(|r| r.0 < r.1);
        ranges.sort_unstable();
        let mut s = Spans::default();
        for (a, b) in ranges {
            s.push(a as usize, b as usize);
        }
        s
    }

    /// Adds `[s, e)`, starting at or after every range's start.
    fn push(&mut self, s: usize, e: usize) {
        if s >= e {
            return;
        }
        let (s, e) = (u(s), u(e));
        match self.0.last_mut() {
            Some(l) if s < l.0 => {
                let mut v = std::mem::take(&mut self.0);
                v.push((s, e));
                *self = Spans::of(v);
            }
            Some(l) if s <= l.1 => l.1 = l.1.max(e),
            _ => self.0.push((s, e)),
        }
    }

    /// The end of the range holding `k`.
    fn end_of(&self, k: usize) -> Option<usize> {
        let i = self.0.partition_point(|r| r.0 as usize <= k);
        let r = self.0.get(i.checked_sub(1)?)?;
        (k < r.1 as usize).then_some(r.1 as usize)
    }

    fn has(&self, k: usize) -> bool {
        self.end_of(k).is_some()
    }

    fn union(&self, o: &Spans) -> Spans {
        Spans::of(self.0.iter().chain(&o.0).copied().collect())
    }
}

struct Analysis<'t> {
    text: &'t str,
    p: Pass,
    defs: Definitions,
    /// Item index by atom.
    item_of: FxHashMap<usize, usize>,
    /// Atoms an earlier item claims (operands: never run on their own).
    claimed: Spans,
    /// Atoms textually inside an alignment's braces (`\\halign{`…`}`).
    in_align: Spans,
    /// The braces' atoms, each with the brace depth TeX's text has at
    /// it (a `}` after its close) and after it: the depth of every atom
    /// ([`Analysis::depth`]). Atoms inside a brace group of the text: a
    /// font they select is the group's, restored at its `}` (§1217),
    /// never a definition of the node.
    braces: Vec<(u32, u32, u32)>,
    /// The atoms a span or a slice reads: every one but a space and a
    /// character after the first of its run.
    sig: Vec<u32>,
    /// The control sequences' and braces' atoms, in order.
    structural: Vec<u32>,
    memo: std::cell::RefCell<FxHashMap<String, Arc<DefSet>>>,
    /// The heads each pair of call lists adds
    /// ([`Analysis::calls_state_split`]), by the lists' hash, with the
    /// lists.
    /// What each set adds to a bound ([`Analysis::add_set_parts`]), by
    /// the set (kept alive here).
    #[allow(clippy::type_complexity)]
    parts_memo: std::cell::RefCell<FxHashMap<usize, (Arc<DefSet>, std::rc::Rc<Bound>)>>,
    #[allow(clippy::type_complexity)]
    split_memo:
        std::cell::RefCell<FxHashMap<u64, (Vec<String>, Vec<String>, Option<Vec<&'static str>>)>>,
    /// The names whose state [`Analysis::defined_state`] is reading.
    visiting: std::cell::RefCell<Vec<String>>,
    /// The format's definitions, behind the text's own.
    prelude: Defs,
    /// The hash of each of the pass's regimes' content (a run's key
    /// across texts, [`run_hint`]).
    reg_hash: Vec<u64>,
    /// The characters the text's and the format's definitions make
    /// active, looked up at the first character asked about (the read
    /// noted then, as at every later one: one lookup, not one per
    /// character of every run, [`Analysis::is_active`]).
    active: std::cell::Cell<Option<u128>>,
}

/// Names that start a paragraph in vertical mode (§1090): the text
/// after them is in one.
fn starts_par(name: &str) -> bool {
    matches!(
        name,
        "indent"
            | "noindent"
            | "char"
            | "unhbox"
            | "unhcopy"
            | "vrule"
            | "accent"
            | "discretionary"
            | "-"
            | "hskip"
            | "hfil"
            | "hfill"
            | "hss"
            | "hfilneg"
            | "valign"
            | " "
            | "noboundary"
    )
}

impl Analysis<'_> {
    /// Whether the items from atom `k0` to byte `e` all end before `e`
    /// by a known extent, and none ends the job.
    fn slice_claims_nothing(&self, k0: usize, e: usize) -> bool {
        self.slice_claims_nothing_(k0, e, true)
    }

    /// [`Analysis::slice_claims_nothing`]; `par_end` false for a slice
    /// that starts no paragraph (`\\centerline{…}`, `\\hsize=6in` in
    /// vertical mode): it
    /// need not end in a paragraph end, and its macros' extents may be
    /// the pass's predictions.
    fn slice_claims_nothing_(&self, k0: usize, e: usize, par_end: bool) -> bool {
        let atoms = self.atoms();
        let k1 = atoms.partition_point(|x| x.start < e);
        let i0 = self.p.items.partition_point(|it| it.atom < k0);
        // (the slice must end in a paragraph end of its own text, a blank
        // line or `\\par`: otherwise a paragraph begun here reads on into
        // the siblings until something ends it, §1094, and a command at
        // its end that scans for a keyword or unit, `\\medskip`'s
        // `\\vskip`, reads the next sibling's first token, §407)
        if par_end {
            let last = (k0..k1).rev().find(|&a| atoms[a].kind != AtomKind::Space);
            if !last.is_some_and(|a| self.ends_par_at(a)) {
                return false;
            }
        }
        for it in self.p.items[i0..].iter().take_while(|it| it.atom < k1) {
            // (a macro's extent from its meaning in the format, the
            // pass's prediction, is taken here: the engine checks the
            // claim against the run and refutes it where the run read on)
            let known = match it.hints.claim {
                pass::Claim::Known(_) => true,
                pass::Claim::Predicted { .. } => !par_end,
                pass::Claim::Pending { .. } => false,
            };
            if it.end > k1 || !known {
                return false;
            }
            if let Some(at) = atoms.get(it.atom)
                && at.kind == AtomKind::Cs
            {
                let n = at.name(self.text);
                let d = self.defset(n);
                if matches!(n, "end" | "dump" | "bye")
                    || d.calls.iter().any(|c| matches!(c.as_str(), "end" | "dump"))
                {
                    return false;
                }
                // (without a paragraph end: only a command whose every
                // call the pass sees, none built by name, as an ending
                // of the job may be; and none that looks at the token
                // after it, `\futurelet`, §1221, which is the next
                // sibling's)
                if !par_end
                    && (!d.unbounded.is_empty()
                        || n == "futurelet"
                        || d.calls.iter().any(|c| c == "futurelet"))
                {
                    return false;
                }
            }
        }
        true
    }

    /// Whether the atom at `a` ends a paragraph (a blank line, `\\par`,
    /// a command the pass marks paragraph-ending).
    fn ends_par_at(&self, a: usize) -> bool {
        let at = &self.p.atoms[a];
        match at.kind {
            AtomKind::Par => true,
            AtomKind::Cs => {
                matches!(at.name(self.text), "par" | "tex_par:D")
                    || self.item_of.get(&a).is_some_and(|&i| {
                        self.p.items[i].hints.implicit == pass::Implicit::EndsParagraph
                    })
            }
            _ => false,
        }
    }

    #[cfg(test)]
    fn new(text: &str, kind: FileKind, prelude: Defs) -> Analysis<'_> {
        Analysis::with_own(text, kind, prelude, own_definitions(text, kind))
    }

    /// [`Analysis::new`] given the text's own definitions
    /// ([`own_definitions`]).
    fn with_own(text: &str, kind: FileKind, prelude: Defs, defs: Definitions) -> Analysis<'_> {
        Analysis::with_own_from(text, kind, prelude, defs, None)
    }

    /// [`Analysis::with_own`] beginning in the regime `start` (`None`:
    /// `kind`'s own; [`pass::Start`]).
    fn with_own_from<'a>(
        text: &'a str,
        kind: FileKind,
        prelude: Defs,
        defs: Definitions,
        start: Option<&pass::Start>,
    ) -> Analysis<'a> {
        let scope = Scope {
            document: &defs,
            file: 0,
            prelude: &prelude,
        };
        let p = pass::analyze_from(text, kind, scope, pass::Features::default(), start);
        let item_of = p
            .items
            .iter()
            .enumerate()
            .map(|(i, it)| (it.atom, i))
            .collect();
        let n = p.atoms.len();
        let claimed = Spans::of(
            p.items
                .iter()
                .map(|it| {
                    // (a scope opener, `\\hbox to 3pt{`, claims its operands
                    // up to its brace; the list inside runs, §1083, §1086)
                    let end = match (&it.hints.scope, &it.hints.claim) {
                        (pass::ScopeHint::Open(_), pass::Claim::Known(b)) => {
                            let to = p.atoms.get(it.atom).map_or(0, |a| a.end) + b;
                            p.atoms.partition_point(|a| a.start < to).min(it.end)
                        }
                        _ => it.end,
                    };
                    (u(it.atom + 1), u(end.min(n)))
                })
                .collect(),
        );
        // (one linear scan: the atoms that are not plain text, a space or
        // a character inside its run, which no loop below reads)
        let mut sig = Vec::new();
        let mut structural = Vec::new();
        let mut braces = Vec::new();
        let mut d = 0u32;
        let mut prev_char = false;
        for (k, a) in p.atoms.iter().enumerate() {
            let ch = matches!(a.kind, AtomKind::Char(_) | AtomKind::Verb);
            if !((ch && prev_char) || a.kind == AtomKind::Space) {
                sig.push(u(k));
            }
            prev_char = ch;
            match a.kind {
                AtomKind::Open => {
                    braces.push((u(k), d, d + 1));
                    d += 1;
                }
                AtomKind::Close => {
                    d = d.saturating_sub(1);
                    braces.push((u(k), d, d));
                }
                AtomKind::Cs => {}
                _ => continue,
            }
            structural.push(u(k));
        }
        let in_align = align_atoms(text, &p.atoms, &structural);
        let reg_hash = p
            .regimes
            .iter()
            .map(|r| {
                let mut h = phitex_defuse::FxHasher::default();
                std::hash::Hash::hash(&r.1, &mut h);
                std::hash::Hasher::finish(&h)
            })
            .collect();
        Analysis {
            text,
            p,
            defs,
            item_of,
            claimed,
            in_align,
            braces,
            sig,
            structural,
            memo: std::cell::RefCell::new(FxHashMap::default()),
            split_memo: std::cell::RefCell::new(FxHashMap::default()),
            parts_memo: std::cell::RefCell::new(FxHashMap::default()),
            visiting: std::cell::RefCell::new(Vec::new()),
            prelude,
            reg_hash,
            active: std::cell::Cell::new(None),
        }
    }

    fn atoms(&self) -> &[Atom] {
        &self.p.atoms
    }

    fn claimed(&self, k: usize) -> bool {
        self.claimed.has(k)
    }

    /// The brace depth at atom `k` (a `}`'s: after it closes).
    fn depth(&self, k: usize) -> usize {
        let i = self.braces.partition_point(|b| b.0 as usize <= k);
        match i.checked_sub(1).map(|i| self.braces[i]) {
            Some((a, at, _)) if a as usize == k => at as usize,
            Some((_, _, after)) => after as usize,
            None => 0,
        }
    }

    /// Whether atom `k` is inside a brace group of the text.
    fn inner(&self, k: usize) -> bool {
        k < self.p.atoms.len() && self.depth(k) > 0
    }

    /// The index of atom `k`'s regime in the pass's regimes.
    fn reg(&self, k: usize) -> usize {
        let r = &self.p.regimes;
        if r.len() <= 1 {
            return 0;
        }
        let tok = self.p.atoms[k].tok;
        r[1..].partition_point(|x| x.0 <= tok)
    }

    /// Whether the text or the format defines `name`.
    fn is_defined(&self, name: &str) -> bool {
        self.defs.is_defined(name) || self.prelude.is_defined(name)
    }

    /// The shape the text gives `name`, else the format's.
    /// The engine's field for a `param:\\name` the pass names: the
    /// parameter's own (§1228); none for `\\spacefactor` and
    /// `\\prevdepth`, which go on the list (§1243); a register's family
    /// for a name the format allocated (`\\smallskipamount`, §1224);
    /// `None` when the name is none of these.
    #[allow(clippy::option_option)]
    fn param_field(&self, f: &str) -> Option<Option<String>> {
        let n = f.strip_prefix("param:")?;
        let n = n.strip_prefix('\\').unwrap_or(n);
        if table_param(n) {
            return Some(Some(cat(&["param:", n])));
        }
        if matches!(n, "spacefactor" | "prevdepth") {
            return Some(None);
        }
        match self.shape(n) {
            Some(pass::Shape::Quantity('i')) => Some(Some("count:".to_owned())),
            Some(pass::Shape::Quantity('d')) => Some(Some("dimen:".to_owned())),
            Some(pass::Shape::Quantity('g')) => Some(Some("skip:".to_owned())),
            Some(pass::Shape::Quantity('t')) => Some(Some("toks:".to_owned())),
            _ => None,
        }
    }

    fn shape(&self, name: &str) -> Option<pass::Shape> {
        self.defs.shape(name).or_else(|| self.prelude.shape(name))
    }

    /// What calling `name` (as the text, then the format, defines it)
    /// may define.
    fn defset(&self, name: &str) -> Arc<DefSet> {
        if let Some(d) = self.memo.borrow().get(name) {
            return d.clone();
        }
        let d = self.defs.call_defines(&self.prelude, name);
        self.memo.borrow_mut().insert(name.to_owned(), d.clone());
        d
    }

    /// The bytes an atom is read from, with the catcodes of its regime:
    /// a control word's escape and letters (a control symbol's character
    /// has any catcode but a letter's), a character, a brace, a space.
    /// Nothing of an atom read under another regime than the command's
    /// first (a change the command itself makes, as the pass predicted).
    fn cats_of(&self, first: usize, a: usize, out: &mut Cats) {
        let Some(at) = self.atoms().get(a) else {
            return;
        };
        // (the regimes by index, computed once per text: no compare of
        // two catcode tables)
        if first < self.p.atoms.len() && self.reg(first) != self.reg(a) {
            return;
        }
        let r = &self.p.regimes[self.reg(a)].1;
        let t = self.text.as_bytes();
        let bytes: &[u8] = match at.kind {
            AtomKind::Cs => {
                let name = at.name(self.text);
                // (where the name ends is read too: the character after a
                // control word, or a control symbol's own, is no letter,
                // or the name would go on, §356)
                let next = if name.bytes().all(|b| r.catcode(b) == Some(11)) {
                    t.get(at.end).copied()
                } else {
                    t.get(at.start + 1).copied()
                };
                if let Some(b) = next
                    && b < 128
                    && (b == 9 || (32..127).contains(&b))
                    && r.catcode(b).is_some_and(|c| c != 11)
                {
                    out.add((b, NOT_LETTER));
                }
                if name.bytes().all(|b| r.catcode(b) == Some(11)) {
                    &t[at.start..at.end]
                } else {
                    &t[at.start..=at.start]
                }
            }
            AtomKind::Char(_) | AtomKind::Open | AtomKind::Close | AtomKind::Param(_) => {
                &t[at.start..at.end]
            }
            AtomKind::Space => b" ",
            AtomKind::Par | AtomKind::Verb => &[],
        };
        for &b in bytes {
            if b < 128
                && (b == 9 || (32..127).contains(&b))
                && let Some(c) = r.catcode(b)
            {
                out.add((b, c));
            }
        }
    }

    /// The atom before `a` that is not a space.
    fn prev(&self, a: usize) -> Option<usize> {
        let mut k = a;
        while k > 0 {
            k -= 1;
            if self.atoms()[k].kind != AtomKind::Space {
                return Some(k);
            }
        }
        None
    }

    /// Whether horizontal material at `a` may start a paragraph: it is
    /// in one only after a character or a command that started it.
    fn may_start_par(&self, a: usize) -> bool {
        let Some(mut k) = self.prev(a) else {
            return true;
        };
        // (an operand an earlier command read, `\vsize=2.5in`, is not
        // text: the command before it decides)
        while self.claimed(k) {
            match self.prev(k) {
                Some(p) => k = p,
                None => return true,
            }
        }
        match self.atoms()[k].kind {
            AtomKind::Char(_) | AtomKind::Verb | AtomKind::Param(_) => false,
            AtomKind::Cs => !starts_par(self.atoms()[k].name(self.text)),
            _ => true,
        }
    }

    /// Whether `\afterassignment` stands just before the command at `a`
    /// (with its token): the assignment consumes `aftertoken` (§1269).
    fn after_afterassignment(&self, a: usize) -> bool {
        let Some(t) = self.prev(a) else { return false };
        let Some(c) = self.prev(t) else { return false };
        let at = self.atoms()[c];
        at.kind == AtomKind::Cs
            && matches!(
                at.name(self.text),
                "afterassignment" | "tex_afterassignment:D"
            )
    }

    /// The hint of the command whose first atom is `a`.
    #[allow(clippy::too_many_lines)]
    fn hint(&self, a: usize) -> Hint {
        let atoms = self.atoms();
        let Some(at) = atoms.get(a) else {
            return Hint::any("end of text", "");
        };
        let excerpt = excerpt_at(self.text, at.start);
        if self.claimed(a) {
            return Hint::any(
                "operand of an earlier command (claimed, never run)",
                excerpt,
            );
        }
        self.hint_run(a)
    }

    /// Whether the atom at `k`, outside every brace group of the text
    /// (or after `\\global`), is a command whose own state (its class,
    /// or its body's calls, not its arguments') includes `head`.
    fn selects_outside(&self, k: usize, head: &str) -> bool {
        let atoms = self.atoms();
        let Some(at) = atoms.get(k) else { return false };
        if at.kind != AtomKind::Cs || (self.inner(k) && !self.after_global(k)) {
            return false;
        }
        let name = at.name(self.text);
        let mut st = Some(Vec::new());
        if self.is_defined(name) {
            self.defined_state(name, false, &mut st);
        } else {
            add_heads(&mut st, class_state(name));
            if class_state(name).is_none() && (is_prim(name) || table_param(name)) {
                return true;
            }
        }
        st.is_none_or(|st| st.contains(&head))
    }

    /// Whether the atom at `a` follows `\\global` (spaces between).
    fn after_global(&self, a: usize) -> bool {
        let atoms = self.atoms();
        let mut k = a;
        while k > 0 {
            k -= 1;
            match atoms[k].kind {
                AtomKind::Space => {}
                AtomKind::Cs => return atoms[k].name(self.text) == "global",
                _ => return false,
            }
        }
        false
    }

    /// The hint of the atom at `a` as if it ran (a command a macro is
    /// only guessed to read does run, when the guess is wrong).
    #[allow(clippy::too_many_lines)]
    fn hint_run(&self, a: usize) -> Hint {
        let atoms = self.atoms();
        let Some(at) = atoms.get(a) else {
            return Hint::any("end of text", "");
        };
        let excerpt = excerpt_at(self.text, at.start);
        if at.kind == AtomKind::Cs && DEMOTED.contains(&at.name(self.text)) {
            return demoted(at.name(self.text), excerpt);
        }
        if at.kind != AtomKind::Cs
            && let Some(p) = self.prev(a)
            && self.atoms()[p].kind == AtomKind::Cs
            // (a command that is itself an operand, `\\footnote\\dag{…}`,
            // reads nothing: its reader does)
            && !self.claimed(p)
            && {
                let h = self.hint(p);
                h.bound.is_none() && !is_demoted(&h)
            }
        {
            // (what follows a command the pass cannot bound may be its
            // operand, read by it)
            return Hint::any("may be the operand of an unbounded command", excerpt);
        }
        let mut b = Bound {
            state: Some(Vec::new()),
            ..Bound::default()
        };
        if self.in_align.has(a) {
            add_head(&mut b.state, "align");
        }
        let mut cause = String::new();
        let ok = match at.kind {
            AtomKind::Cs => self.cs_hint(a, a, &mut b, &mut cause),
            AtomKind::Char(_) | AtomKind::Verb => {
                // (a run of characters: the active ones expand here, and
                // the token after the run is looked at, §1038)
                // (the run goes on through the commands after it in its
                // own text, and a line's end may end the paragraph there:
                // `\\prevgraf`, §1096)
                add_head(&mut b.state, "pg:");
                let mut k = a;
                let mut ok = true;
                while let Some(x) = atoms.get(k)
                    && matches!(x.kind, AtomKind::Char(_) | AtomKind::Verb)
                {
                    self.cats_of(a, k, &mut b.cats);
                    if x.kind == AtomKind::Char('$') {
                        // (math begins: its list's opener and `\\fam`
                        // set to -1, §1138, §1139; a display ends the
                        // paragraph before it and sets its sizes, §1145,
                        // §1146)
                        add_heads(&mut b.state, Some(MATH_SHIFT));
                    }
                    if x.kind == AtomKind::Char('&')
                        && self.p.regime_at(x.tok).catcode(b'&') == Some(4)
                    {
                        // (a cell's end, §789)
                        add_head(&mut b.state, "align");
                        add_head(&mut b.state, "badness");
                    }
                    if let AtomKind::Char(c) = x.kind
                        && self.is_active(c)
                    {
                        ok = ok && self.call_hint(&c.to_string(), &mut b, &mut cause);
                    }
                    k += 1;
                }
                if ok && atoms.get(k).is_some_and(|x| x.kind == AtomKind::Cs) {
                    ok = self.cs_hint(a, k, &mut b, &mut cause);
                }
                ok
            }
            AtomKind::Par => {
                self.cats_of(a, a, &mut b.cats);
                add_heads(&mut b.state, Some(PAR_END));
                self.call_hint("par", &mut b, &mut cause)
            }
            AtomKind::Close => {
                self.cats_of(a, a, &mut b.cats);
                add_heads(&mut b.state, class_state("egroup"));
                true
            }
            AtomKind::Open | AtomKind::Space | AtomKind::Param(_) => {
                self.cats_of(a, a, &mut b.cats);
                true
            }
        };
        // (a relied name the set itself defines is not an assumption:
        // plain's `\\footnote` does `\\let\\@sf\\empty`, so `cs:@sf`
        // need not be defined at the call)
        {
            let (names, prefixes) = (&b.names, &b.prefixes);
            b.means
                .retain(|m| !names.contains(&m.0) && !prefixes.iter().any(|p| m.0.starts_with(p)));
        }
        if let Some(f) = b.names.iter().find(|f| {
            f.strip_prefix("act:")
                .and_then(|c| c.parse::<u32>().ok())
                .and_then(char::from_u32)
                .is_some_and(|c| !self.is_active(c))
        }) {
            // (a character this text never makes active is defined as
            // one: the regime here is not the one the pass read)
            return Hint::any(
                cat(&[f, ": an active character the text does not make"]),
                excerpt,
            );
        }
        // (a font selected inside a brace group, not `\\global`ly, is
        // the group's: no `font` definition reaches the engine)
        if self.inner(a)
            && !self.after_global(a)
            && let Some(st) = &mut b.state
        {
            st.retain(|h| *h != "font");
        }
        b.no_scope = ok && self.no_scope(a);
        if ok {
            if cause.is_empty() {
                "bounded".clone_into(&mut cause);
            }
            Hint {
                bound: Some(b),
                cause,
                excerpt: short(excerpt),
                guess: None,
            }
        } else {
            let mut h = Hint::any(cause, excerpt);
            if at.kind == AtomKind::Cs {
                self.guess_call(at.name(self.text), &mut b);
                h.guess = Some(b);
            }
            h
        }
    }

    /// Whether the command at `a` opens and closes no scope: a run of
    /// characters without math shifts or active characters, read up to a
    /// command that is itself such; a space; a primitive assignment,
    /// definition or message (§1208-§1280), never a macro, a box, a group
    /// or a conditional.
    /// For a run of characters starting at `a` whose hint depends only
    /// on its own text, its regime and its place (no active character,
    /// no command after it or before it, all under one regime, not an
    /// operand): its text's span, the regime and the place's flags (in
    /// an alignment, inside a group without `\\global`). Runs with equal
    /// keys get equal hints ([`Analysis::hint_run`]).
    fn run_key(&self, a: usize) -> Option<(usize, usize, usize, u8)> {
        let atoms = self.atoms();
        let at = atoms.get(a)?;
        if !matches!(at.kind, AtomKind::Char(_) | AtomKind::Verb) || self.claimed(a) {
            return None;
        }
        if self.prev(a).is_some_and(|p| atoms[p].kind == AtomKind::Cs) {
            return None;
        }
        let mut k = a;
        while let Some(x) = atoms.get(k)
            && matches!(x.kind, AtomKind::Char(_) | AtomKind::Verb)
        {
            if self.atoms()[k].tok != at.tok && self.reg(k) != self.reg(a) {
                return None;
            }
            if let AtomKind::Char(c) = x.kind
                && self.is_active(c)
            {
                return None;
            }
            k += 1;
        }
        if atoms.get(k).is_some_and(|x| x.kind == AtomKind::Cs) {
            return None;
        }
        let flags = u8::from(self.in_align.has(a))
            | (u8::from(self.inner(a) && !self.after_global(a)) << 1);
        Some((at.start, atoms[k - 1].end, self.reg(a), flags))
    }

    fn no_scope(&self, a: usize) -> bool {
        let atoms = self.atoms();
        match atoms[a].kind {
            AtomKind::Space => true,
            AtomKind::Char(_) | AtomKind::Verb => {
                let mut k = a;
                while let Some(x) = atoms.get(k)
                    && matches!(x.kind, AtomKind::Char(_) | AtomKind::Verb)
                {
                    if let AtomKind::Char(c) = x.kind
                        && (self.is_active(c) || matches!(c, '$' | '&' | '^' | '_' | '#'))
                    {
                        return false;
                    }
                    k += 1;
                }
                match atoms.get(k) {
                    Some(x) if x.kind == AtomKind::Cs => self.cs_no_scope(k),
                    _ => true,
                }
            }
            AtomKind::Cs => self.cs_no_scope(a),
            _ => false,
        }
    }

    fn cs_no_scope(&self, a: usize) -> bool {
        let name = self.atoms()[a].name(self.text);
        if self.is_defined(name) {
            return false;
        }
        let n = name
            .strip_prefix("tex_")
            .and_then(|x| x.strip_suffix(":D"))
            .unwrap_or(name);
        matches!(
            n,
            "def"
                | "gdef"
                | "edef"
                | "xdef"
                | "let"
                | "chardef"
                | "mathchardef"
                | "countdef"
                | "dimendef"
                | "skipdef"
                | "muskipdef"
                | "toksdef"
                | "relax"
                | "message"
                | "errmessage"
                | "catcode"
                | "lccode"
                | "uccode"
                | "sfcode"
                | "mathcode"
                | "delcode"
                | "count"
                | "dimen"
                | "skip"
                | "muskip"
                | "advance"
                | "multiply"
                | "divide"
                | "font"
                | "fontdimen"
                | "hyphenchar"
                | "skewchar"
                | "penalty"
                | "kern"
                | "hskip"
                | "vskip"
                | "hfil"
                | "hfill"
                | "vfil"
                | "vfill"
                | "hss"
                | "vss"
                | "char"
                | "/"
                | " "
                | "unskip"
                | "unpenalty"
                | "unkern"
                | "textfont"
                | "scriptfont"
                | "scriptscriptfont"
                | "showthe"
                | "show"
        ) || (table_param(n) && !matches!(n, "everypar" | "output"))
    }

    fn is_active(&self, c: char) -> bool {
        c == '~'
            || (c.is_ascii() && {
                let m = self.active.get().unwrap_or_else(|| {
                    let m = self.defs.active_chars() | self.prelude.active_chars();
                    self.active.set(Some(m));
                    m
                });
                m >> (c as u32) & 1 == 1
            })
    }

    /// Whether `name` is a format macro the text itself does not define:
    /// its body is read for paragraph regions (`slice_info`) but not
    /// trusted to bound what a call defines, since a definition outside
    /// a bound ends the evaluation (the pass's closure over a kernel body
    /// misses computed names, `\csname` and `\let` behind
    /// `\@ifundefined`); such a call answers `Any` as before.
    /// A macro called by name (`\par` for a blank line, an active
    /// character).
    fn call_hint(&self, name: &str, b: &mut Bound, cause: &mut String) -> bool {
        let set = self.defset(name);
        let field = match name.chars().next() {
            Some(c) if name.chars().count() == 1 && !c.is_alphabetic() && name != "par" => {
                Tok::ch(13, u32::from(c)).field_name()
            }
            _ => Tok::cs(&caret_name(name)).field_name(),
        };
        if let Some(f) = field {
            b.means.push((f, 1));
        }
        self.add_set(&set, b, cause)
    }

    /// The state the commands `calls` (of a body the text defines) may
    /// set.
    fn calls_state_split(
        &self,
        calls: &[String],
        outer: &[String],
        st: &mut Option<Vec<&'static str>>,
    ) {
        if st.is_none() {
            return;
        }
        // (the heads a pair of call lists adds, made once per pair in the
        // analysis and added to each state that asks; not while a name's
        // state is being read, where a recursion cuts it short)
        if self.visiting.borrow().is_empty() {
            let key = {
                let mut h = phitex_defuse::FxHasher::default();
                calls.hash(&mut h);
                outer.hash(&mut h);
                h.finish()
            };
            // (a hit is checked against the lists themselves)
            let known = self
                .split_memo
                .borrow()
                .get(&key)
                .filter(|e| e.0 == calls && e.1 == outer)
                .map(|e| e.2.clone());
            let d = known.unwrap_or_else(|| {
                let mut d = Some(Vec::new());
                self.calls_state_split_(calls, outer, &mut d);
                self.split_memo
                    .borrow_mut()
                    .insert(key, (calls.to_vec(), outer.to_vec(), d.clone()));
                d
            });
            match d {
                None => *st = None,
                Some(hs) => add_heads(st, Some(&hs)),
            }
            return;
        }
        self.calls_state_split_(calls, outer, st);
    }

    /// [`Analysis::calls_state_split`], made.
    fn calls_state_split_(
        &self,
        calls: &[String],
        outer: &[String],
        st: &mut Option<Vec<&'static str>>,
    ) {
        // (by a set of the outer calls: no scan of them per call)
        let outer: phitex_defuse::FxHashSet<&str> = outer.iter().map(String::as_str).collect();
        let (out, inner): (Vec<&str>, Vec<&str>) = calls
            .iter()
            .map(String::as_str)
            .partition(|c| outer.contains(c));
        self.calls_state(&out, st);
        if inner.is_empty() || st.is_none() {
            return;
        }
        // (calls inside a group a body opens: what they set locally is
        // undone at its end, §283; the rest of their state counts)
        let mut sub = Some(Vec::new());
        self.calls_state(&inner, &mut sub);
        match sub {
            None => *st = None,
            Some(hs) => {
                let keep: Vec<&str> = hs
                    .iter()
                    .copied()
                    .filter(|h| !group_local_head(h))
                    .collect();
                add_heads(st, Some(&keep));
            }
        }
    }

    fn calls_state(&self, calls: &[&str], st: &mut Option<Vec<&'static str>>) {
        for &c in calls {
            if st.is_none() {
                return;
            }
            if self.is_defined(c) {
                // (a name the text defines: its body's calls are in the
                // list already)
                self.defined_state(c, true, st);
                continue;
            }
            if starts_par(c) {
                add_heads(st, Some(&["pg:"]));
            }
            let row = class_state(c);
            if row.is_none() && !is_prim(c) && !table_param(c) {
                // (a name neither a primitive nor defined: a `\\let`'s
                // value that may be undefined, `\\let\\next\\undefined`;
                // a call of it is unbounded in the may-define set, not
                // in the state)
                if crate::exec::obs_flag!("PHITEX_SLICES_TRACE") {
                    eprintln!("slices: state skips undefined \\{c}");
                }
                continue;
            }
            if row.is_none() && crate::exec::obs_flag!("PHITEX_SLICES_TRACE") {
                eprintln!("slices: state every head from \\{c}");
            }
            if row.is_some_and(|r| r.contains(&"param:"))
                && crate::exec::obs_flag!("PHITEX_SLICES_TRACE")
            {
                eprintln!("slices: param: from the row of \\{c}");
            }
            add_heads(st, row);
        }
    }

    /// The state a name the text defines sets when it runs, besides its
    /// body's calls: a font identifier selects its font (§1217); a
    /// register is assigned (§1224); a character or a `\\chardef` is
    /// appended, or is a brace or tab the `\\let` copied (§1038, §1063,
    /// §1100, §789); an alias is its target's class.
    fn defined_state(&self, name: &str, may_start_par: bool, st: &mut Option<Vec<&'static str>>) {
        match self.shape(name) {
            Some(pass::Shape::Quantity('f')) => add_head(st, "font"),
            // (a register's name assigns that register, §1224: its own
            // family; the value is scanned with expansion, §498, and
            // `\\afterassignment`'s token runs, §1269; never a parameter)
            Some(pass::Shape::Quantity(q @ ('i' | 'd' | 'g' | 't'))) => {
                let fam = match q {
                    'i' => "count:",
                    'd' => "dimen:",
                    'g' => "skip:",
                    _ => "toks:",
                };
                // (a `\countdef` that gave its number: that register)
                let r = if self.defs.shape(name).is_some() {
                    self.defs.register(name)
                } else {
                    self.prelude.register(name)
                };
                match r {
                    Some(r) => add_heads(st, Some(&[head(&format!("{fam}{r}")), "aftertoken"])),
                    None => add_heads(st, Some(&[fam, "aftertoken"])),
                }
            }
            // (a character, or a brace whose `}` sets only the register
            // its `\\setbox` names, §1075, as the `egroup` row)
            Some(pass::Shape::Quantity(_)) => {
                add_heads(st, Some(&["box:255", "lvlfix", "badness", "inspen"]));
                if may_start_par {
                    add_heads(st, Some(&["pg:"]));
                }
            }
            Some(pass::Shape::Alias(t)) => {
                if starts_par(&t) && may_start_par {
                    add_heads(st, Some(&["pg:"]));
                }
                if !self.is_defined(&t) {
                    add_heads(st, class_state(&t));
                }
            }
            Some(pass::Shape::Cond | pass::Shape::Macro(_)) => {}
            // (definitions that disagree, or one the pass cannot read)
            Some(pass::Shape::Opaque(_)) | None => {
                // (definitions that disagree, `\\let\\@sf\\empty` and an
                // `\\edef` of it: what its bodies run, when the pass
                // bounds that and it defines no name of its own)
                let set = self.defset(name);
                // (a name whose bodies run it again, directly or
                // through others: its state is unbounded here, never a
                // recursion without end)
                let cyclic = self.visiting.borrow().iter().any(|v| v == name);
                if cyclic {
                    *st = None;
                    return;
                }
                if set.unbounded.is_empty() && st.is_some() {
                    self.visiting.borrow_mut().push(name.to_owned());
                    let mut b = Bound {
                        state: st.clone(),
                        ..Bound::default()
                    };
                    let mut c = String::new();
                    let ok = self.add_set(&set, &mut b, &mut c)
                        && b.names.is_empty()
                        && b.prefixes.is_empty();
                    self.visiting.borrow_mut().pop();
                    if ok {
                        *st = b.state;
                        return;
                    }
                }
                if st.is_some() && crate::exec::obs_flag!("PHITEX_SLICES_TRACE") {
                    eprintln!(
                        "slices: state every head from defined \\{name}: {:?}",
                        self.shape(name)
                    );
                }
                *st = None;
            }
        }
    }

    /// The set's fields and relied macros into the bound.
    fn add_set(&self, set: &Arc<DefSet>, b: &mut Bound, cause: &mut String) -> bool {
        if let Some(u) = set.unbounded.first() {
            cause.clone_from(u);
            return false;
        }
        self.add_set_parts(set, b);
        true
    }

    /// The guess for a call the pass cannot bound: what its bodies
    /// define as far as the pass sees them, and its primitive's row.
    fn guess_call(&self, name: &str, b: &mut Bound) {
        if is_prim(name) {
            add_heads(&mut b.state, class_state(name));
        } else {
            let set = self.defset(name);
            self.add_set_parts(&set, b);
        }
    }

    /// A definition set's names, prefixes and calls' state, whether or
    /// not the pass bounds it.
    fn add_set_parts(&self, set: &Arc<DefSet>, b: &mut Bound) {
        // (what a set adds, made once per set in the analysis (its sets
        // are kept in `memo`, each by its name) and added to each bound
        // that asks, each entry once; not while a name's state is being
        // read, where a recursion cuts it short)
        if !self.visiting.borrow().is_empty() {
            return self.add_set_parts_(set, b);
        }
        let key = Arc::as_ptr(set) as usize;
        let known = self.parts_memo.borrow().get(&key).map(|e| e.1.clone());
        let d = known.unwrap_or_else(|| {
            let mut d = Bound {
                state: Some(Vec::new()),
                ..Bound::default()
            };
            self.add_set_parts_(set, &mut d);
            let d = std::rc::Rc::new(d);
            self.parts_memo
                .borrow_mut()
                .insert(key, (set.clone(), d.clone()));
            d
        });
        let means: phitex_defuse::FxHashSet<&str> = b.means.iter().map(|m| m.0.as_str()).collect();
        let new_means: Vec<(String, u8)> = d
            .means
            .iter()
            .filter(|m| !means.contains(m.0.as_str()))
            .cloned()
            .collect();
        b.means.extend(new_means);
        let names: phitex_defuse::FxHashSet<&str> = b.names.iter().map(String::as_str).collect();
        let new_names: Vec<String> = d
            .names
            .iter()
            .filter(|n| !names.contains(n.as_str()))
            .cloned()
            .collect();
        b.names.extend(new_names);
        let prefixes: phitex_defuse::FxHashSet<&str> =
            b.prefixes.iter().map(String::as_str).collect();
        let new_prefixes: Vec<String> = d
            .prefixes
            .iter()
            .filter(|p| !prefixes.contains(p.as_str()))
            .cloned()
            .collect();
        b.prefixes.extend(new_prefixes);
        match &d.state {
            None => b.state = None,
            Some(hs) => add_heads(&mut b.state, Some(hs)),
        }
    }

    /// [`Analysis::add_set_parts`], made.
    fn add_set_parts_(&self, set: &DefSet, b: &mut Bound) {
        // (what the bound has already, by name: each name added once
        // without a scan of the lists per name)
        let mut means: phitex_defuse::FxHashSet<String> =
            b.means.iter().map(|m| m.0.clone()).collect();
        for r in &set.relied {
            if let Some(f) = cs_field(r)
                && !means.contains(&f)
            {
                means.insert(f.clone());
                b.means.push((f, 1));
            }
        }
        let mut names: phitex_defuse::FxHashSet<String> = b.names.iter().cloned().collect();
        for f in set.names.keys() {
            if let Some(f) = engine_field(f) {
                if !names.contains(&f) {
                    names.insert(f.clone());
                    b.names.push(f);
                }
            } else if let Some(p) = self.param_field(f) {
                if let Some(p) = p {
                    add_head_dyn(&mut b.state, &p);
                }
            } else if let Some(h) = head_of_pass_field(f) {
                if crate::exec::obs_flag!("PHITEX_SLICES_TRACE") {
                    eprintln!("slices: head {h} from name {f}");
                }
                if register_by_number(f) {
                    // (a register by its number, the page number's
                    // `count:0`: that register, not its whole family)
                    add_head_dyn(&mut b.state, f);
                } else {
                    add_head(&mut b.state, h);
                }
            }
        }
        let mut prefixes: phitex_defuse::FxHashSet<String> = b.prefixes.iter().cloned().collect();
        for f in set.prefixes.keys() {
            if let Some(p) = engine_prefix(f) {
                if !prefixes.contains(&p) {
                    prefixes.insert(p.clone());
                    b.prefixes.push(p);
                }
            } else if let Some(p) = self.param_field(f) {
                if let Some(p) = p {
                    add_head_dyn(&mut b.state, &p);
                }
            } else if let Some(h) = head_of_pass_field(f) {
                if crate::exec::obs_flag!("PHITEX_SLICES_TRACE") {
                    eprintln!("slices: head {h} from prefix {f}");
                }
                add_head(&mut b.state, h);
            }
        }
        self.calls_state_split(&set.calls, &set.outer_calls, &mut b.state);
    }

    /// The hint of the control sequence at `a`, of a command beginning
    /// at `first`.
    #[allow(clippy::too_many_lines)]
    fn cs_hint(&self, first: usize, a: usize, b: &mut Bound, cause: &mut String) -> bool {
        let atoms = self.atoms();
        let name = atoms[a].name(self.text);
        self.cats_of(first, a, &mut b.cats);
        if is_prefix(name) {
            // (the prefixed command runs here)
            let mut k = a + 1;
            while let Some(x) = atoms.get(k) {
                match x.kind {
                    AtomKind::Space => {}
                    AtomKind::Cs if is_prefix(x.name(self.text)) => {}
                    AtomKind::Cs => return self.cs_hint(first, k, b, cause),
                    _ => break,
                }
                self.cats_of(first, k, &mut b.cats);
                k += 1;
            }
            // (`\global` then an active character, or the command in
            // text this one does not see)
            *cause = cat(&["\\", name, ": a prefix whose command follows elsewhere"]);
            return false;
        }
        if LOOKTHROUGH.contains(&name) {
            *cause = cat(&["\\", name, ": the command run here is computed"]);
            return false;
        }
        let Some(&i) = self.item_of.get(&a) else {
            // (an operand of an earlier command: claimed, never run)
            *cause = cat(&["\\", name, ": operand of an earlier command"]);
            return false;
        };
        let it = &self.p.items[i];
        let end = it.end.max(a + 1).min(atoms.len());
        #[allow(clippy::needless_range_loop)]
        for k in a + 1..end {
            self.cats_of(first, k, &mut b.cats);
            // (an operand expanded while read: a pdfTeX primitive's state,
            // a `\\csname` that makes its name `\\relax`, §372)
            if atoms[k].kind == AtomKind::Cs {
                let o = atoms[k].name(self.text);
                if o.starts_with("pdf") && is_prim(o) {
                    add_head(&mut b.state, "pdf:");
                } else if matches!(o, "csname" | "tex_csname:D")
                    && !b.prefixes.iter().any(|p| p == "cs:")
                {
                    b.prefixes.push("cs:".to_owned());
                }
            }
        }
        if let Some(f) = Tok::cs(&caret_name(name)).field_name() {
            b.means.push((f, 1));
        }
        {
            // (a pdfTeX primitive read on the command's line, in an operand
            // the item does not cover: its state, and a box it takes)
            let at = &atoms[a];
            let line = &self.text[at.start..];
            let line = &line[..line.find('\n').unwrap_or(line.len())];
            if line.contains("\\pdf") {
                add_heads(&mut b.state, Some(&["pdf:", "box:"]));
            }
        }
        for r in &it.hints.relied {
            if let Some(f) = cs_field(r)
                && !b.means.iter().any(|m| m.0 == f)
            {
                b.means.push((f, 1));
            }
        }
        // (the state: the command's own class, the text's definition of
        // it, and what the bodies it relies on call)
        if self.is_defined(name) {
            // (a body may open a group of its own)
            self.defined_state(name, self.may_start_par(a), &mut b.state);
        } else {
            add_heads(&mut b.state, class_state(name));
        }
        if name == "font" {
            // (§1257: the font identifier's frozen twin)
            b.prefixes.push("cs:\u{10FFFE}F".to_owned());
        }
        if starts_par(name) && self.may_start_par(a) {
            add_heads(&mut b.state, Some(&["pg:"]));
        }
        if matches!(name, "par" | "tex_par:D") || it.hints.implicit == pass::Implicit::EndsParagraph
        {
            // (a paragraph's end: its lines, `\\prevgraf`, §1096;
            // `normal_paragraph` resets `\\looseness`, `\\hangindent`,
            // `\\hangafter`, `\\parshape`, `\\interlinepenalties`, §1070)
            add_heads(&mut b.state, Some(PAR_END));
        }
        if self.after_afterassignment(first) {
            add_head(&mut b.state, "aftertoken");
        }
        self.calls_state_split(&it.hints.calls, &it.hints.outer_calls, &mut b.state);
        // (a macro's run expands its body inline: the state of every
        // command its body calls, transitively, the closure's calls)
        if !is_prim(name) {
            let set = self.defset(name);
            self.calls_state_split(&set.calls, &set.outer_calls, &mut b.state);
        }
        match &it.hints.may_define {
            pass::MayDefine::Nothing => true,
            pass::MayDefine::Names(ds) | pass::MayDefine::Prefixes(ds) => {
                add_defs_with(
                    self,
                    ds,
                    matches!(it.hints.may_define, pass::MayDefine::Prefixes(_)),
                    b,
                );
                // (a definer the format defines, `\newcommand`: the pass
                // read its target from the call; what the format's bodies
                // define besides, `\let\pr@tectedrel@x\relax`, is the
                // catalogue's closure of the call, whose one computed
                // target is that name)
                if !self.defs.is_defined(name) && self.prelude.is_defined(name) {
                    let set = self.defset(name);
                    let other: Vec<&String> = set
                        .unbounded
                        .iter()
                        .filter(|u| {
                            !u.contains("with a computed name")
                                && !u.contains("with a computed target")
                        })
                        .collect();
                    if let Some(u) = other.first() {
                        *cause = cat(&["\\", name, ": ", u]);
                        return false;
                    }
                    self.add_set_parts(&set, b);
                }
                true
            }
            pass::MayDefine::Any
                if !self.defs.is_defined(name) && self.prelude.is_defined(name) && {
                    // (the format's definition is final: its body's
                    // closure bounds the call)
                    let set = self.defset(name);
                    set.unbounded.is_empty() && self.add_set(&set, b, cause)
                } =>
            {
                true
            }
            pass::MayDefine::Any => {
                *cause = it.hints.cause.map_or_else(
                    || cat(&["\\", name, ": unbounded"]),
                    |c| cat(&["\\", name, ": ", c]),
                );
                if self.prelude.is_defined(name) {
                    // (why the format's body did not bound it)
                    let set = self.defset(name);
                    *cause = cat(&[
                        cause,
                        " (format closure: ",
                        set.unbounded.first().map_or("bounded", String::as_str),
                        ")",
                    ]);
                }
                if *cause == cat(&["\\", name, ": unbounded"]) {
                    // (the reason the pass gives: the closure's)
                    let set = self.defset(name);
                    if let Some(u) = set.unbounded.first() {
                        cause.clone_from(u);
                    }
                }
                false
            }
        }
    }
}

/// Which atoms are inside an alignment's braces: from the `{` after
/// `\\halign` or `\\valign` (spec and keywords between) to its `}`.
fn align_atoms(text: &str, atoms: &[Atom], structural: &[u32]) -> Spans {
    // (only a command or a brace changes it: the atoms between keep the
    // value of the one before)
    let mut out = Spans::default();
    let mut from: Option<usize> = None;
    let mut depth = 0usize;
    let mut open: Vec<usize> = Vec::new();
    let mut pending = false;
    for &k in structural {
        let k = k as usize;
        let a = &atoms[k];
        match a.kind {
            AtomKind::Cs
                if matches!(
                    a.name(text),
                    "halign" | "valign" | "tex_halign:D" | "tex_valign:D"
                ) =>
            {
                pending = true;
            }
            AtomKind::Open => {
                depth += 1;
                if pending {
                    open.push(depth);
                    pending = false;
                }
            }
            AtomKind::Close => {
                if open.last() == Some(&depth) {
                    open.pop();
                }
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
        match (from, open.is_empty()) {
            (None, false) => from = Some(k),
            (Some(f), true) => {
                out.push(f, k);
                from = None;
            }
            _ => {}
        }
    }
    if let Some(f) = from {
        out.push(f, atoms.len());
    }
    out
}

fn add_defs_with(an: &Analysis<'_>, ds: &[Def], prefixes: bool, b: &mut Bound) {
    for d in ds {
        if let Some(p) = an.param_field(&d.field) {
            if let Some(p) = p {
                add_head_dyn(&mut b.state, &p);
            }
        } else if let Some(h) = head_of_pass_field(&d.field) {
            if crate::exec::obs_flag!("PHITEX_SLICES_TRACE") {
                eprintln!("slices: head {h} from def {}", d.field);
            }
            if register_by_number(&d.field) {
                // (a register by its number, the page number's
                // `count:0`: that register, not its whole family)
                add_head_dyn(&mut b.state, &d.field);
            } else {
                add_head(&mut b.state, h);
            }
        } else if prefixes {
            if let Some(p) = engine_prefix(&d.field)
                && !b.prefixes.contains(&p)
            {
                b.prefixes.push(p);
            }
        } else if let Some(f) = engine_field(&d.field)
            && !b.names.contains(&f)
        {
            b.names.push(f);
        }
    }
}

#[cfg(test)]
fn analyse(text: &str, kind: FileKind, defs: &Defs) -> TextHints {
    analyse_with(text, kind, defs, own_definitions(text, kind), None)
}

thread_local! {
    /// The hints of runs of characters by their key ([`Analysis::run_key`]
    /// with the regime's content for its index): a pure function of the
    /// key, so one chunk's run serves the others (without excerpts, the
    /// traces', the hint is the same value).
    static RUN_BOUNDS: std::cell::RefCell<FxHashMap<(u64, u64, u8), Arc<Hint>>> =
        std::cell::RefCell::default();
}

/// The hint of the run of characters at atom `a` (`key`: its text's
/// hash, its regime's index and its place's flags, [`Analysis::run_key`]),
/// its bound shared with every chunk that has the run under the same
/// regime ([`RUN_BOUNDS`]).
#[inline(never)]
fn run_hint(an: &Analysis<'_>, a: usize, key: (u64, usize, u8), start: usize) -> Arc<Hint> {
    let reg = an.reg_hash.get(key.1).copied().unwrap_or(0);
    let gkey = (key.0, reg, key.2);
    let excerpt = short(excerpt_at(an.text, start));
    if let Some(h) = RUN_BOUNDS.with(|c| c.borrow().get(&gkey).cloned()) {
        // (the cached hint, its excerpt this run's)
        if h.excerpt == excerpt {
            return h;
        }
        return Arc::new(Hint {
            excerpt,
            ..(*h).clone()
        });
    }
    let h = an.hint(a);
    // (kept when it has the shape the hit rebuilds: bounded, no guess)
    let keep =
        h.bound.is_some() && h.guess.is_none() && h.cause == "bounded" && h.excerpt == excerpt;
    let h = Arc::new(h);
    if keep {
        RUN_BOUNDS.with(|c| {
            let mut c = c.borrow_mut();
            if c.len() >= 1 << 16 {
                c.clear();
            }
            c.insert(gkey, h.clone());
        });
    }
    h
}

/// The hint of every atom a span or a slice reads (`an.sig`): where it
/// starts, its hint, and whether a slice's union skips it.
#[inline(never)]
#[allow(clippy::type_complexity)]
fn atom_hints(
    an: &Analysis<'_>,
    text: &str,
    reuse: Option<(&TextHints, usize, usize, isize)>,
) -> (Vec<u32>, Vec<Option<Arc<Hint>>>, Vec<bool>, Vec<u64>, usize) {
    let mut starts = Vec::with_capacity(an.sig.len());
    let mut hints: Vec<Option<Arc<Hint>>> = Vec::with_capacity(an.sig.len());
    let mut skip = Vec::with_capacity(an.sig.len());
    let mut sigs = Vec::with_capacity(an.sig.len());
    let mut kept = 0usize;
    let new_n = an.sig.len();
    // (the atoms a primitive reads: never run on their own; a command
    // a macro is only guessed to read runs in the slice, so it counts)
    let n = an.p.atoms.len();
    let prim_operand = Spans::of(
        an.p.items
            .iter()
            .filter(|it| is_prim(&it.name))
            .map(|it| (u(it.atom + 1), u(it.end.min(n))))
            .collect(),
    );
    let mut runs: FxHashMap<(u64, usize, u8), Arc<Hint>> = FxHashMap::default();
    // (an atom inside a run of characters, or a space, is never read:
    // no span reads it, so it has no entry)
    for (i, &a) in an.sig.iter().enumerate() {
        let a = a as usize;
        let at = &an.p.atoms[a];
        starts.push(u(at.start));
        let prim_op = prim_operand.has(a);
        let sig = atom_sig(an, a, prim_op);
        sigs.push(sig);
        // (an atom before the slices around the edit, or after them,
        // whose structure is the same: its hint as the last analysis
        // made it, [`Reuse`])
        if let Some((old, lo, hi, shift)) = reuse {
            let j = if at.start < lo {
                Some(i)
            } else if at.start >= hi {
                (i + old.sigs.len()).checked_sub(new_n)
            } else {
                None
            };
            if let Some(j) = j
                && old.sigs.get(j) == Some(&sig)
                && old.starts.get(j).is_some_and(|&s| s as isize + if at.start < lo { 0 } else { shift } == at.start as isize)
                && let Some(h) = old.hints[j].clone()
            {
                hints.push(Some(h));
                skip.push(old.skip[j]);
                kept += 1;
                continue;
            }
        }
        // (a run of characters whose hint depends on its text alone: one
        // hint per distinct run, shared)
        if let Some((s0, e0, reg, fl)) = an.run_key(a) {
            let key = (
                phitex_defuse::hash_bytes(0x7275_6e73, &text.as_bytes()[s0..e0]),
                reg,
                fl,
            );
            let h = runs
                .entry(key)
                .or_insert_with(|| run_hint(an, a, key, at.start))
                .clone();
            hints.push(Some(h));
            skip.push(false);
            continue;
        }
        let claimed = an.claimed(a);
        let h = if claimed && at.kind == AtomKind::Cs && !prim_op {
            an.hint_run(a)
        } else {
            an.hint(a)
        };
        // (a guessed operand that is unbounded on its own is a value
        // read, not run: `\\headline={\\tenrm…}`)
        let unbounded_operand = claimed && h.bound.is_none();
        hints.push(Some(Arc::new(h)));
        // (a command a macro is only guessed to read still runs in the
        // slice: its rows count in the union)
        skip.push(claimed && (at.kind != AtomKind::Cs || prim_op || unbounded_operand));
    }
    (starts, hints, skip, sigs, kept)
}

/// What the hint of atom `a` depends on in the pass's structure, apart
/// from its own text and the definitions: its kind and length, whether
/// an earlier command claims it or a primitive reads it, its brace depth
/// and its mate's distance, whether it is in an alignment or follows
/// `\global`, its regime, and its item's extent ([`atom_hints`]'s reuse:
/// the same text with the same signature has the same hint).
fn atom_sig(an: &Analysis<'_>, a: usize, prim_op: bool) -> u64 {
    let at = &an.p.atoms[a];
    let mut h = phitex_defuse::FxHasher::default();
    (std::mem::discriminant(&at.kind), at.end - at.start, an.claimed(a), prim_op, an.depth(a), an.in_align.has(a), an.after_global(a)).hash(&mut h);
    (at.mate != usize::MAX).then(|| at.mate.wrapping_sub(a)).hash(&mut h);
    an.reg_hash.get(an.reg(a)).hash(&mut h);
    an.item_of.get(&a).map(|&i| an.p.items[i].end.wrapping_sub(a)).hash(&mut h);
    // (the atom before it: a command's hint reads its neighbour's)
    an.prev(a).map(|p| (std::mem::discriminant(&an.p.atoms[p].kind), an.claimed(p))).hash(&mut h);
    h.finish()
}

/// [`analyse`] given the text's own definitions ([`own_definitions`]).
// (one pass over the atoms, then one over the slices; splitting it
// would only pass its tables around)
#[allow(clippy::too_many_lines)]
fn analyse_with(
    text: &str,
    kind: FileKind,
    defs: &Defs,
    own: Definitions,
    start: Option<&pass::Start>,
) -> TextHints {
    analyse_reusing(text, kind, defs, own, start, None)
}

/// [`analyse_with`] keeping the hints of the atoms an edit did not reach
/// ([`Reuse`]): those before the slice before the edit's, and after the
/// slice after it, whose structure is the same ([`atom_sig`]).
#[allow(clippy::too_many_lines)]
fn analyse_reusing(
    text: &str,
    kind: FileKind,
    defs: &Defs,
    own: Definitions,
    start: Option<&pass::Start>,
    reuse: Option<&Reuse<'_>>,
) -> TextHints {
    // (`PHITEX_TEX_TIME`: the analysis's phases, on the timeline)
    let tm = crate::exec::prof_flag!("PHITEX_TEX_TIME");
    let t0 = std::time::Instant::now();
    let an = Analysis::with_own_from(text, kind, defs.clone(), own, start);
    if tm {
        crate::timeline::note("hints: the pass (lex, atoms, classify)", t0.elapsed());
    }
    let t0 = std::time::Instant::now();
    let (slices, pars) = slices_of(text, &an, kind);
    if tm {
        crate::timeline::note("hints: the slices", t0.elapsed());
    }
    let t0 = std::time::Instant::now();
    // (one value per distinct hint, keyed by the hash of its encoding:
    // no string keys in the pass, spec-final §5.2)
    let mut memo: FxHashMap<u64, Arc<str>> = FxHashMap::default();
    let mut intern = |h: &Hint| -> Arc<str> {
        let s = h.encode();
        memo.entry(phitex_defuse::hash_bytes(0x6869_6e74, s.as_bytes()))
            .or_insert_with(|| Arc::from(s))
            .clone()
    };
    // (the bytes around the edit: from the start of the slice before
    // the one it begins in to the end of the slice after the one it ends
    // in)
    let window = reuse.map(|r| {
        let (a, b, c) = r.edit;
        let k0 = slices.partition_point(|&(_, e)| e <= a).saturating_sub(1);
        let k1 = (slices.partition_point(|&(_, e)| e < c) + 1).min(slices.len().saturating_sub(1));
        let lo = slices.get(k0).map_or(0, |x| x.0);
        let hi = slices.get(k1).map_or(text.len(), |x| x.1);
        #[allow(clippy::cast_possible_wrap)]
        let shift = c as isize - b as isize;
        (r.th, lo, hi, shift)
    });
    let (starts, hints, skip, sigs, kept) = atom_hints(&an, text, window);
    if tm && reuse.is_some() {
        eprintln!("hints reuse: {kept} of {} atoms' hints kept", sigs.len());
    }
    // (observe, `PHITEX_HINTS_REUSE_CHECK=1`: the kept hints against the
    // ones made afresh)
    if window.is_some() && crate::exec::obs_flag!("PHITEX_HINTS_REUSE_CHECK") {
        let (_, fresh, fskip, _, _) = atom_hints(&an, text, None);
        let bad = hints
            .iter()
            .zip(&fresh)
            .zip(skip.iter().zip(&fskip))
            .filter(|((x, y), (s, t))| x.as_ref().map(|h| h.encode()) != y.as_ref().map(|h| h.encode()) || s != t)
            .count();
        eprintln!("hints reuse check: {kept} of {} kept, {bad} differ", fresh.len());
    }
    let mut t = TextHints {
        slices,
        starts,
        hints,
        skip,
        pars,
        slice_hints: Vec::new(),
        sigs,
    };
    // each slice's hint: the union of its commands' bounds (a run
    // executes the whole slice); a paragraph's region also defines
    // `\prevgraf` of the list it is in
    let mut slice_hints = Vec::with_capacity(t.slices.len());
    let mut pending: Vec<&'static str> = Vec::new();
    // (which of `NORMAL_PARAGRAPH` a hint may set, by hint: the atoms of
    // a run of characters share theirs)
    let mut sets: FxHashMap<usize, u8> = FxHashMap::default();
    for (i, &(s, e)) in t.slices.iter().enumerate() {
        let mut h = t.at_until(s, e);
        // (whether the run reads on into the next slice: a scan for a
        // keyword or unit there, below; its claim is then the run's)
        let mut reads_on = false;
        // (where such a scan stops for sure: a blank line's `\\par` in a
        // blanks-only slice after it, non-expandable, §407; the slice
        // index and its end)
        let mut scan_stop: Option<(usize, usize)> = None;
        // (a slice ending in a command that scans on for a keyword or a
        // unit, `\\vskip 1in`, expands what follows while it looks, and
        // runs what that leaves, §1094, §407: the next slice's commands
        // run in this node when it starts with a macro, or with another
        // such scanning command, transitively)
        {
            // (`lastk`: the last slice taken in that is not blanks only)
            let (mut j, mut lastk) = (i, i);
            while let (Some(last), Some(&(ns, ne))) =
                (last_item(&an.p, text, t.slices[lastk]), t.slices.get(j + 1))
            {
                if !SCANS_ON.contains(&last.as_str()) {
                    break;
                }
                let Some(first) = first_cs(&an.p, text, (ns, ne)) else {
                    // (blanks between: the scan reads past them, up to a
                    // blank line, a `\\par`, where it stops: the run
                    // takes the siblings to that one, no more)
                    if text[ns..ne].trim().is_empty() {
                        reads_on = true;
                        j += 1;
                        let atoms = an.atoms();
                        let a0 = atoms.partition_point(|x| x.start < ns);
                        if atoms[a0..]
                            .iter()
                            .take_while(|x| x.start < ne)
                            .any(|x| x.kind == AtomKind::Par)
                        {
                            scan_stop = Some((j, ne));
                            break;
                        }
                        continue;
                    }
                    break;
                };
                let next = t.at_until(ns, ne);
                reads_on = true;
                match (&mut h.bound, &next.bound) {
                    (Some(a), Some(b)) => union(a, b),
                    (Some(_), None) => h = next.clone(),
                    (None, _) => match (
                        h.guess.as_mut(),
                        next.bound.as_ref().or(next.guess.as_ref()),
                    ) {
                        (Some(g), Some(b)) => union(g, b),
                        _ => h.guess = None,
                    },
                }
                if is_prim(&first) && !SCANS_ON.contains(&first.as_str()) {
                    break;
                }
                if !is_prim(&first) {
                    break;
                }
                j += 1;
                lastk = j;
            }
        }
        // (a document slice is balanced by construction: its groups,
        // paragraphs and conditionals are whole inside it; when its
        // braces and named group commands pair up and no macro it calls
        // opens or closes a group its body does not, its run opens and
        // closes no scope of the engine's)
        if kind == FileKind::Document
            && let Some(b) = &mut h.bound
            && balanced(&an, text, s, e)
        {
            b.no_scope = true;
        }
        let excerpt = short(excerpt_at(text, s));
        // (a font selection, `\\bf`, that only an argument or a group
        // inside the slice makes is local to that group: the slice
        // declares it only when a command outside every brace group of
        // the slice selects one, §1217)
        // (the slice's atoms, `k0..k1`; of them only a command selects)
        let k0 = an.atoms().partition_point(|x| x.start < s);
        let k1 = an.atoms().partition_point(|x| x.start < e);
        let cs = &an.structural[an.structural.partition_point(|&x| (x as usize) < k0)
            ..an.structural.partition_point(|&x| (x as usize) < k1)];
        for head in GROUP_LOCAL {
            let outside = cs.iter().any(|&k| an.selects_outside(k as usize, head));
            if !outside {
                for b in [h.bound.as_mut(), h.guess.as_mut()].into_iter().flatten() {
                    if let Some(st) = &mut b.state {
                        st.retain(|x| *x != head);
                    }
                }
            }
        }
        if t.pars[i] {
            if let Some(b) = &mut h.bound {
                add_heads(&mut b.state, Some(&["pg:"]));
                "region".clone_into(&mut h.cause);
            } else {
                h.cause.insert_str(0, "region: ");
                if let Some(g) = &mut h.guess {
                    add_head(&mut g.state, "pg:");
                }
            }
            h.excerpt = excerpt;
        }
        // (a reset by `normal_paragraph`, §1070, defines its parameter
        // only when a command set it since the last paragraph end: a
        // slice that may set one and ends no paragraph after it leaves
        // the reset pending, and the next slice ending a paragraph
        // declares it)
        let mut out = pending.clone();
        let mut ends = false;
        // (a space or a character inside its run ends nothing and sets
        // nothing: the entries of the atoms read only)
        for a in t.starts.partition_point(|&x| (x as usize) < s)..t.starts.len() {
            if t.starts[a] as usize >= e {
                break;
            }
            if an.ends_par_at(an.sig[a] as usize) {
                ends = true;
                out.clear();
            }
            if t.skip[a] {
                continue;
            }
            let Some(ah) = t.hints[a].as_deref() else {
                continue;
            };
            let m = *sets
                .entry(std::ptr::from_ref(ah) as usize)
                .or_insert_with(|| {
                    NORMAL_PARAGRAPH
                        .iter()
                        .enumerate()
                        .filter(|&(_, n)| may_set(ah, n))
                        .fold(0, |m, (k, _)| m | 1 << k)
                });
            for (k, &n) in NORMAL_PARAGRAPH.iter().enumerate() {
                if m >> k & 1 == 1 && !out.contains(&n) {
                    out.push(n);
                }
            }
        }
        if ends && let Some(b) = h.bound.as_mut().or(h.guess.as_mut()) {
            // (a paragraph ended anywhere in the slice, a blank line in
            // a macro's argument too (a runaway, §396): its `\\prevgraf`,
            // §1096; and the resets pending, §1070)
            add_head(&mut b.state, "pg:");
            add_heads(&mut b.state, Some(&pending));
        }
        pending = out;
        // (the siblings the run takes: none when every item of the slice
        // ends inside it by the pass's known extent, its groups balance
        // and nothing ends the job, `\\end` taking the rest, §1335)
        if kind == FileKind::Document
            && let Some(b) = &mut h.bound
            && b.no_scope
        {
            // (a slice that does not end its paragraph, or ends in a
            // command scanning on, followed by a blank line alone: its
            // run takes that sibling, whose `\\par` ends both, and no
            // more)
            let par_next = t.slices.get(i + 1).copied().filter(|&(ns, ne)| {
                text[ns..ne].trim().is_empty() && {
                    let atoms = an.atoms();
                    let a0 = atoms.partition_point(|x| x.start < ns);
                    atoms[a0..]
                        .iter()
                        .take_while(|x| x.start < ne)
                        .any(|x| x.kind == AtomKind::Par)
                }
            });
            b.claim = match scan_stop {
                Some((n, end)) => an.slice_claims_nothing(k0, end).then_some(n - i),
                None if reads_on => None,
                None if an.slice_claims_nothing(k0, e) => Some(0),
                // (a slice that starts no paragraph and whose commands
                // all end inside it by the pass's extents, the last one
                // scanning on into nothing (`reads_on` above), `\hsize=6in`,
                // `\centerline{…}`: nothing it runs reads the next; a run
                // that does take a sibling refutes this, and its
                // followers run again)
                None if !t.pars[i] && an.slice_claims_nothing_(k0, e, false) => {
                    if crate::exec::obs_flag!("PHITEX_SLICES_TRACE") {
                        eprintln!(
                            "slices: claims nothing, no paragraph end: {:?}",
                            short(&text[s..e])
                        );
                    }
                    Some(0)
                }
                None => par_next
                    .filter(|&(_, ne)| an.slice_claims_nothing(k0, ne))
                    .map(|_| 1),
            };
        }
        slice_hints.push(intern(&h));
    }
    // (`PHITEX_HINTS_DIGEST`: one line per text, a hash of every hint
    // a span reads (the atoms not skipped) and every slice's, encoded:
    // the check that a rework of the analysis keeps its answers byte
    // for byte)
    if crate::exec::obs_flag!("PHITEX_HINTS_DIGEST") {
        let mut d = phitex_defuse::hash_bytes(0, text.as_bytes());
        for (h, &skip) in t.hints.iter().zip(&t.skip) {
            if !skip && let Some(h) = h {
                d = phitex_defuse::hash_bytes(d, h.encode().as_bytes());
            }
        }
        for h in &slice_hints {
            d = phitex_defuse::hash_bytes(d, h.as_bytes());
        }
        eprintln!(
            "hints digest: {d:016x} ({} atoms, {} slices)",
            t.hints.len(),
            slice_hints.len()
        );
    }
    if crate::exec::obs_flag!("PHITEX_SLICES_TRACE") {
        let open = slice_hints
            .iter()
            .filter(|h| !Hint::decode(h).bound.is_some_and(|b| b.no_scope))
            .count();
        eprintln!("slices: no_scope false: {open} of {}", slice_hints.len());
        for (k, h) in slice_hints.iter().enumerate() {
            let d = Hint::decode(h);
            if !d.bound.as_ref().is_some_and(|b| b.no_scope) {
                let (s, e) = t.slices[k];
                eprintln!(
                    "slices: scope {:?}: {} ({}, balanced {})",
                    short(&text[s..e]),
                    d.cause,
                    if d.bound.is_some() { "bounded" } else { "any" },
                    balanced(&an, text, s, e)
                );
            }
            if d.bound
                .as_ref()
                .is_some_and(|b| b.no_scope && b.claim.is_none())
            {
                let (s, e) = t.slices[k];
                let atoms = an.atoms();
                let (k0, k1) = (
                    atoms.partition_point(|x| x.start < s),
                    atoms.partition_point(|x| x.start < e),
                );
                let i0 = an.p.items.partition_point(|it| it.atom < k0);
                let items: Vec<String> = an.p.items[i0..]
                    .iter()
                    .take_while(|it| it.atom < k1)
                    .map(|it| {
                        let c = match &it.hints.claim {
                            pass::Claim::Known(n) => format!("known {n}"),
                            pass::Claim::Predicted { n_tokens, .. } => {
                                format!("predicted {n_tokens}")
                            }
                            pass::Claim::Pending { cause } => format!("pending ({cause})"),
                        };
                        format!("{}: {c}, end {} of {k1}", atoms[it.atom].name(text), it.end)
                    })
                    .collect();
                eprintln!(
                    "slices: claim pending {:?}: starts a paragraph {}, {}; items {items:?}",
                    short(&text[s..e]),
                    t.pars[k],
                    d.cause
                );
            }
        }
    }
    t.slice_hints = slice_hints;
    if tm {
        crate::timeline::note("hints: the atoms' and slices' hints", t0.elapsed());
    }
    t
}

// ---------------------------------------------------------------------
// Caches.

fn hash_str(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    s.len().hash(&mut h);
    h.finish()
}

type Cache<K> = Mutex<FxHashMap<K, Arc<TextHints>>>;

/// By the text, its kind and the definitions, by address. The entry
/// holds a weak reference to the definitions it was made under: an
/// `Arc`'s allocation never moves and is freed only when its strong and
/// weak counts are both zero, so while the entry lives no other set can
/// be allocated at that address (a set dropped is a miss: its `Weak`
/// no longer upgrades). Keyed by the bare address, a document chunk's
/// chained definitions (`incr::Incr::chain`, made per build and dropped
/// with it) could be followed by another set at the same address, which
/// would get the analysis made under the first.
/// (`phitex_pdf::digest::blob_hash` memoizes the same way.)
type TextsCache = Mutex<
    FxHashMap<(u64, u8, usize, Option<u64>), (std::sync::Weak<pass::Definitions>, Arc<TextHints>)>,
>;

fn texts() -> &'static TextsCache {
    static C: OnceLock<TextsCache> = OnceLock::new();
    C.get_or_init(|| Mutex::new(FxHashMap::default()))
}

/// A build's hint state (`run::Ops::hints`): the analyses of texts
/// registered by name, and the nodes' hints by their arguments. Owned
/// by the build, not the process: every document is registered under
/// the same name (`api::DOC_NAME`) and an `\\input` file by its path,
/// so a registry shared by the builds of a process gave a node the
/// analysis of another build's document at its offsets.
#[derive(Default)]
pub(crate) struct Registry {
    by_name: Mutex<FxHashMap<u64, Arc<TextHints>>>,
    /// The hint of each unit of a file `\\input` reads as units
    /// (`api::file_units`), by the unit's field name (a string value):
    /// the analysis of the whole file at the unit's slice.
    units: Mutex<FxHashMap<u64, Arc<str>>>,
    hints: Mutex<FxHashMap<u64, Arc<Hint>>>,
    /// The engine's answers ([`may_define`]) made ahead on the pool for
    /// nodes about to be pushed ([`prepare`]), by their arguments' key:
    /// each taken by the push's one question, the rest dropped after it
    /// ([`prepared_done`]).
    prepared: Mutex<FxHashMap<u64, Prepared>>,
    /// The hints decoded, by their string's value (the nodes of a
    /// document share a few hint strings among thousands of slices).
    decoded: Mutex<FxHashMap<u64, Arc<Hint>>>,
    /// [`may_define`]'s answer by the hint string's value.
    answers: Mutex<FxHashMap<u64, MayDefine>>,
}

/// An answer kept for its one question, or taken.
#[derive(Default)]
enum Kept<T> {
    Ready(T),
    #[default]
    Taken,
}

/// A node's answers made before its push ([`prepare`]), each taken by
/// the engine's one question.
#[derive(Default)]
struct Prepared {
    may: Option<MayDefine>,
    reads: Kept<Option<Vec<Field>>>,
    lines: Option<ValueId>,
}

/// Registers a text's analysis under `name` (a string value) in the
/// build's registry: a node made from part of that text later (the rest
/// of a slice another command's read-ahead claimed, `Run::leftover`)
/// names it and its offset, and gets the union hint of its text from
/// the whole analysis instead of one computed on its own without the
/// definitions.
pub(crate) fn register_text(reg: &Registry, name: ValueId, t: &Arc<TextHints>) {
    let mut bn = reg.by_name.locked();
    if !bn.get(&name.raw()).is_some_and(|x| Arc::ptr_eq(x, t)) {
        bn.insert(name.raw(), t.clone());
    }
}

/// Registers the hint of a file's unit (`key`, its field name as a
/// string value): a node whose text is that unit (a `["w", key, ...]`
/// frame) gets it from here, as the unit's slice of the whole file's
/// analysis.
pub(crate) fn register_unit(reg: &Registry, key: ValueId, hint: Arc<str>) {
    reg.units.locked().insert(key.raw(), hint);
}

/// Withdraws the analysis registered under `name` (a document whose
/// text is no longer one analysed whole): its nodes' hints answer
/// `Any`, as before anything was registered.
pub(crate) fn unregister_text(reg: &Registry, name: ValueId) {
    reg.by_name.locked().remove(&name.raw());
}

/// Whether a text is written for INITEX's catcodes: its first `{` comes
/// after its own `\catcode` of `{` (plain.tex, latex.ltx, a document
/// run without a format).
fn looks_initex(text: &str) -> bool {
    // (the first `\catcode` of `{` written with a backquote, else the
    // first written with its code; found in one pass over `\catcode`s)
    let (mut quoted, mut coded) = (None, None);
    let mut from = 0;
    while let Some(k) = text[from..].find("\\catcode") {
        let p = from + k;
        let rest = &text[p + "\\catcode".len()..];
        if rest.starts_with("`\\{") {
            quoted = Some(p);
            break;
        }
        if coded.is_none() && rest.starts_with("123") {
            coded = Some(p);
        }
        from = p + 1;
    }
    let Some(line) = quoted.or(coded) else {
        return false;
    };
    text.find('{').is_none_or(|b| line < b)
}

/// The regime a text starts in: the kind its name says, unless the text
/// itself sets up its braces.
pub(crate) fn kind_of(text: &str, named: FileKind) -> FileKind {
    if looks_initex(text) {
        FileKind::Initex
    } else {
        named
    }
}

/// The hint of an output routine's text (`\output` shown as source,
/// `crate::pages`), read whole under the format's definitions: what
/// the finisher running it may define.
pub(crate) fn routine_hint(text: &str, defs: &Defs) -> Hint {
    let t = text_hints(text, FileKind::Document, defs);
    let mut h = t.at_until(0, text.len());
    if crate::exec::obs_flag!("PHITEX_HINTS") {
        eprintln!(
            "hint (output routine) {:?}: {:?} ({})",
            short(text),
            h.may_define(),
            h.cause
        );
    }
    h.excerpt = short(text);
    h
}

/// The analysis of a document's or prelude's text under the format's
/// definitions `defs` (cached by the text and the definitions).
pub(crate) fn text_hints(text: &str, kind: FileKind, defs: &Defs) -> Arc<TextHints> {
    text_hints_with(text, kind, defs, None, None)
}

/// The definitions a text makes on its own (harvested from it alone,
/// read in the regime `kind` names, [`kind_of`]): what its analysis sees
/// as the text's, and whether it defines anything
/// ([`TextHints::defines`], [`defines_any`]), without the definitions
/// in force before it.
pub(crate) fn own_definitions(text: &str, kind: FileKind) -> Definitions {
    own_definitions_from(text, kind, None).0
}

/// [`own_definitions`] of a text beginning in the regime `start`
/// (`None`: its kind's own; [`pass::Start`]), with the regime it ends in.
pub(crate) fn own_definitions_from(
    text: &str,
    kind: FileKind,
    start: Option<&pass::Start>,
) -> (Definitions, pass::Start) {
    let mut d = Definitions::new();
    let end = d.harvest_from(text, kind_of(text, kind), 0, start);
    (d, end)
}

/// Whether a text's own definitions ([`own_definitions`]) define
/// anything (a macro, an active character).
pub(crate) fn defines_any(own: &Definitions) -> bool {
    !own.is_empty() || own.active_chars() != 0
}

/// [`text_hints`], given the text's own definitions when they are
/// harvested already ([`own_definitions`] of the same text and kind).
pub(crate) fn text_hints_with(
    text: &str,
    kind: FileKind,
    defs: &Defs,
    own: Option<Definitions>,
    start: Option<&pass::Start>,
) -> Arc<TextHints> {
    let named = kind;
    let kind = kind_of(text, kind);
    let key = (
        hash_str(text),
        kind as u8,
        Arc::as_ptr(defs) as usize,
        start.map(|s| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            s.hash(&mut h);
            h.finish()
        }),
    );
    if let Some((d, t)) = texts().locked().get(&key)
        && d.upgrade().is_some_and(|d| Arc::ptr_eq(&d, defs))
    {
        return t.clone();
    }
    let own = own.unwrap_or_else(|| own_definitions_from(text, named, start).0);
    let t = Arc::new(analyse_with(text, kind, defs, own, start));
    let mut c = texts().locked();
    if c.len() > 256 {
        c.clear();
    }
    c.insert(key, (Arc::downgrade(defs), t.clone()));
    t
}

/// [`text_hints_reads`] keeping the hints of the atoms the edit did not
/// reach ([`Reuse`]). The names looked up are only those of the atoms
/// analysed again: the caller adds the last analysis's.
pub(crate) fn text_hints_reads_reusing(
    text: &str,
    kind: FileKind,
    defs: &Defs,
    own: Option<Definitions>,
    start: Option<&pass::Start>,
    reuse: &Reuse<'_>,
) -> (Arc<TextHints>, Vec<String>) {
    let named = kind;
    let kind = kind_of(text, kind);
    let own = own.unwrap_or_else(|| own_definitions_from(text, named, start).0);
    let (t, looked) = pass::with_reads(|| analyse_reusing(text, kind, defs, own, start, Some(reuse)));
    (Arc::new(t), looked)
}

/// [`text_hints_with`] made afresh (no cache), with the names the
/// analysis looked up in the definitions ([`pass::with_reads`]): a
/// document chunk's analysis and its read set (`crate::incr`).
pub(crate) fn text_hints_reads(
    text: &str,
    kind: FileKind,
    defs: &Defs,
    own: Option<Definitions>,
    start: Option<&pass::Start>,
) -> (Arc<TextHints>, Vec<String>) {
    let named = kind;
    let kind = kind_of(text, kind);
    let own = own.unwrap_or_else(|| own_definitions_from(text, named, start).0);
    let (t, looked) = pass::with_reads(|| analyse_with(text, kind, defs, own, start));
    (Arc::new(t), looked)
}

/// The hint of the document slice beginning at byte `s` of the text `t`
/// analyses (looked up once per text by the caller), encoded.
pub(crate) fn slice_hint_of(t: &TextHints, s: usize) -> Arc<str> {
    match t.slice_hint(s) {
        Some(h) => h.clone(),
        None => Hint::any("slice not found", "").encode().into(),
    }
}

/// [`slice_hint_of`] as the node's extra argument.
pub(crate) fn hint_arg(v: &Values, t: &TextHints, s: usize) -> ValueId {
    v.str(&slice_hint_of(t, s))
}

fn by_id() -> &'static Cache<u64> {
    static C: OnceLock<Cache<u64>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(FxHashMap::default()))
}

/// [`crate::inputs::document_text`] of the text value `id`, known once
/// per text (ids are the content's: a test made each time a file is
/// reached scanned the whole text, a copy of it first).
pub(crate) fn document_text_of(v: &Values, id: ValueId) -> bool {
    static C: OnceLock<Mutex<FxHashMap<u64, bool>>> = OnceLock::new();
    let c = C.get_or_init(|| Mutex::new(FxHashMap::default()));
    if let Some(&b) = c.locked().get(&id.raw()) {
        return b;
    }
    let b = crate::inputs::document_text(&str_of(v, id));
    c.locked().insert(id.raw(), b);
    b
}

/// The analysis of a file read by `\input`: its text value `id`, named
/// `name`.
pub(crate) fn file_hints(reg: &Registry, v: &Values, id: ValueId, name: ValueId) -> Arc<TextHints> {
    let known = by_id().locked().get(&id.raw()).cloned();
    #[allow(clippy::single_match_else)]
    let t = match known {
        Some(t) => t,
        None => {
            let text = str_of(v, id);
            let nm = str_of(v, name);
            // (a file read by `\input`: analysed without the format's
            // definitions, which the frame does not name; a call it
            // cannot bound answers `Any`, never a narrower set)
            let t = text_hints(&text, FileKind::from_path(&nm), no_defs());
            by_id().locked().insert(id.raw(), t.clone());
            t
        }
    };
    register_text(reg, name, &t);
    t
}

/// The key of a node's arguments in the registry's tables.
fn args_key(args: &[ValueId]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for a in args {
        a.raw().hash(&mut h);
    }
    h.finish()
}

/// The answers the engine asks of a node at its push, made now (on the
/// pool, for the nodes of a text before they are pushed in order: the
/// push's serial part is then the engine's own, spec-final §9b): its
/// hint decoded and cached, its [`may_define`] kept until the push asks.
pub(crate) fn prepare(reg: &Registry, v: &Values, args: &[ValueId]) {
    let _ = hint_of_args(reg, v, args);
    let p = Prepared {
        // (`PHITEX_HINTS` prints each answer where the engine asks)
        may: (!crate::exec::obs_flag!("PHITEX_HINTS")).then(|| may_define_now(reg, v, args)),
        reads: Kept::Ready(reads_now(reg, v, args)),
        lines: Some(crate::run::text_lines(v, args)),
    };
    reg.prepared.locked().insert(args_key(args), p);
}

/// The answer `f` takes from a node's prepared ones, if it was prepared.
fn take_prepared<T>(
    reg: &Registry,
    args: &[ValueId],
    f: impl FnOnce(&mut Prepared) -> Option<T>,
) -> Option<T> {
    let mut p = reg.prepared.locked();
    if p.is_empty() {
        return None;
    }
    p.get_mut(&args_key(args)).and_then(f)
}

/// The lines a node's text begins (`crate::run::text_lines`), as
/// prepared before its push.
pub(crate) fn prepared_lines(reg: &Registry, args: &[ValueId]) -> Option<ValueId> {
    take_prepared(reg, args, |p| p.lines.take())
}

/// The push the answers were [`prepare`]d for is done: those it did not
/// take are dropped (a later node with the same arguments asks again).
pub(crate) fn prepared_done(reg: &Registry) {
    reg.prepared.locked().clear();
}

/// The hint of a node from its arguments: the extra argument when the
/// slice carries one, else its first text frame.
pub(crate) fn hint_of_args(reg: &Registry, v: &Values, args: &[ValueId]) -> Arc<Hint> {
    // (the hints off: every node's answers `Any`, its claim, effect and
    // reads unknown, [`set_hints_enabled`])
    if OFF.load(Ordering::Relaxed) {
        static ANY: OnceLock<Arc<Hint>> = OnceLock::new();
        return ANY
            .get_or_init(|| Arc::new(Hint::any("hints off", "")))
            .clone();
    }
    let key = args_key(args);
    if let Some(h) = reg.hints.locked().get(&key) {
        return h.clone();
    }
    let mut registry = false;
    let h = match hint_value(v, args) {
        // (the hint argument's string decoded once per build)
        Some(hv) => {
            let known = reg.decoded.locked().get(&hv.raw()).cloned();
            known.unwrap_or_else(|| {
                let h = Arc::new(compute_hint(reg, v, args, &mut registry));
                let mut d = reg.decoded.locked();
                if d.len() > 1 << 16 {
                    d.clear();
                }
                d.insert(hv.raw(), h.clone());
                h
            })
        }
        None => Arc::new(compute_hint(reg, v, args, &mut registry)),
    };
    if registry {
        // (a hint read from the registered analysis of a text, which an
        // edit registers again: the arguments alone do not name it)
        return h;
    }
    let mut c = reg.hints.locked();
    if c.len() > 1 << 20 {
        c.clear();
    }
    c.insert(key, h.clone());
    h
}

/// The node's hint argument, the string tagged `A` or `B` (what
/// [`compute_hint`] decodes first).
fn hint_value(v: &Values, args: &[ValueId]) -> Option<ValueId> {
    args.iter().skip(2).copied().find(|&h| {
        matches!(&*v.get(h), Value::Str(t) if t.starts_with("A\x1e") || t.starts_with("B\x1e"))
    })
}

#[allow(clippy::too_many_lines)]
fn compute_hint(reg: &Registry, v: &Values, args: &[ValueId], registry: &mut bool) -> Hint {
    // (the hint argument: the string tagged `A` or `B`, wherever the
    // node's other extra arguments put it)
    for &h in args.iter().skip(2) {
        if let Value::Str(t) = &*v.get(h)
            && (t.starts_with("A\x1e") || t.starts_with("B\x1e"))
        {
            return Hint::decode(t);
        }
    }
    let Some(&fr) = args.first() else {
        return Hint::any("no arguments", "");
    };
    let frames = list_of(v, fr);
    let Some(&f) = frames.first() else {
        return Hint::any("no text", "");
    };
    let xs = list_of(v, f);
    let tag = xs.first().map(|&t| str_of(v, t)).unwrap_or_default();
    match tag.as_str() {
        "f" => {
            if let (Some(&id), Some(&o), Some(&name)) = (xs.get(1), xs.get(2), xs.get(3)) {
                // (the rest of the file: its next line runs whole here,
                // and so do the lines after it while a group or a
                // conditional stays open; the union of the line's
                // commands, extended to the end of such a span)
                let off = usize::try_from(int_of(v, o)).unwrap_or(0);
                let t = file_hints(reg, v, id, name);
                let text = str_of(v, id);
                let end = line_span_end(&text, off);
                return t.at_until(off, end);
            }
            Hint::any("file frame without a position", "")
        }
        "s" => {
            let text = xs.get(1).map(|&t| str_of(v, t)).unwrap_or_default();
            if xs.get(4).is_some_and(|&x| int_of(v, x) & 4 != 0) {
                // (a pseudo file's or a `\read` line: its catcodes are
                // the ones in force when it is read, which its own
                // commands may change, pdfTeX §1766, §483)
                return Hint::any("text read under the catcodes in force", &text);
            }
            if let (Some(&name), Some(&o)) = (xs.get(5), xs.get(6)) {
                let at = usize::try_from(int_of(v, o)).unwrap_or(0);
                // (what the registry holds now: not cached by the
                // arguments, `hint_of_args`)
                *registry = true;
                let t = reg.by_name.locked().get(&name.raw()).cloned();
                return match t {
                    Some(t) => t.at_until(at, at + text.len()),
                    None => Hint::any("file not analysed yet", &text),
                };
            }
            // (the rest of a document slice: read on its own, its
            // catcodes verified when it runs)
            let mut h = text_hints(&text, FileKind::Document, no_defs()).at(0);
            if let Some(b) = &mut h.bound {
                fragment_context(b);
                h.cause.push_str(" (text on its own)");
            }
            h
        }
        "t" => {
            // (expansion tokens: a `}` ending an output routine, a
            // `\\par` inserted: demoted until their rows are proven)
            let toks = crate::val::toks_of(v, xs.get(1).copied());
            // (characters only, letters, others and spaces, already read:
            // they append to the paragraph's list, may begin one (§1091,
            // `\\prevgraf` 0) and open and close no scope; no meaning
            // or catcode is read again, §1030-§1044)
            if !toks.is_empty() && toks.iter().all(|t| matches!(t.cat(), Some(10..=12))) {
                let mut b = Bound {
                    state: Some(Vec::new()),
                    no_scope: true,
                    ..Bound::default()
                };
                // (a run that reaches a cell's end in an alignment, §789:
                // the template's v part runs there)
                add_heads(
                    &mut b.state,
                    Some(&["pg:", "box:", "lvlfix", "align", "badness"]),
                );
                return Hint {
                    bound: Some(b),
                    cause: "characters (expansion tokens)".to_owned(),
                    excerpt: String::new(),
                    guess: None,
                };
            }
            if !crate::exec::obs_flag!("PHITEX_HINTS_FRAGMENTS") {
                return demoted("expansion tokens", "");
            }
            // (tokens already read: faithful as text when every
            // character has the catcode the regime gives it)
            let mut text = String::new();
            let mut kind = None;
            for regime in [FileKind::Document, FileKind::Package] {
                let at = regime == FileKind::Package;
                text.clear();
                let mut ok = true;
                for t in &toks {
                    if let Some(n) = t.as_cs() {
                        // (a name the pass reads back as one token:
                        // letters of the regime, or one other character)
                        let word = !n.is_empty()
                            && n.chars()
                                .all(|c| c.is_ascii_alphabetic() || (at && c == '@'));
                        let symbol = n.chars().count() == 1
                            && n.chars().all(|c| c.is_ascii() && !c.is_ascii_alphabetic());
                        if !(word || symbol) {
                            ok = false;
                            break;
                        }
                        text.push('\\');
                        text.push_str(&n);
                        if word {
                            text.push(' ');
                        }
                    } else {
                        let c = t.chr();
                        let cat = t.cat().unwrap_or(12);
                        let Some(ch) = char::from_u32(c).filter(char::is_ascii) else {
                            ok = false;
                            break;
                        };
                        if plain_catcode(ch as u8, at) != cat {
                            ok = false;
                            break;
                        }
                        text.push(ch);
                    }
                }
                if ok {
                    kind = Some(regime);
                    break;
                }
            }
            match kind {
                Some(k) => {
                    let mut h = text_hints(&text, k, no_defs()).at(0);
                    if let Some(b) = &mut h.bound {
                        // (the tokens are read already)
                        b.cats.clear();
                        fragment_context(b);
                        h.cause.push_str(" (expansion tokens)");
                    }
                    h
                }
                None => Hint::any("expansion tokens with unusual catcodes", &text),
            }
        }
        "w" => {
            // (a file's unit: its slice's hint in the whole file's
            // analysis, registered when the file was cut into units;
            // what the registry holds now, not cached by the arguments)
            *registry = true;
            let h = xs
                .get(1)
                .and_then(|&k| reg.units.locked().get(&k.raw()).cloned());
            match h {
                Some(h) if frames.len() == 1 => Hint::decode(&h),
                _ => Hint::any("unit not analysed", ""),
            }
        }
        _ => Hint::any("unknown frame", ""),
    }
}

/// A fragment's context is not in its text: it may start a paragraph,
/// follow an `\afterassignment`, and the state of what it calls is
/// known only by name.
fn fragment_context(b: &mut Bound) {
    // (a `}` among them may end the output routine: the page's state
    // and the held-over contributions are restored, §1100)
    add_heads(&mut b.state, Some(&["deadcycles"]));
    // (and a box a `\\setbox` before it began, §1075)
    add_head(&mut b.state, "box:");
    add_heads(&mut b.state, Some(NORMAL_PARAGRAPH));
    add_head(&mut b.state, "pg:");
    add_head(&mut b.state, "aftertoken");
}

/// The heads a font switch sets (§1217, §1228 `\\fam`): local to the
/// group it runs in, so a slice declares them only for a switch outside
/// every brace group ([`Analysis::selects_outside`]).
const GROUP_LOCAL: [&str; 2] = ["font", "param:fam"];

/// A head whose fields a local assignment sets and a group's end
/// restores (§283): the parameters, the registers, the current font,
/// `\\parshape`.
fn group_local_head(h: &str) -> bool {
    h == "font"
        || h == "parshape"
        || [
            "param:", "count:", "dimen:", "skip:", "muskip:", "toks:", "box:",
        ]
        .iter()
        .any(|p| h.starts_with(p))
}

/// Plain TeX's catcode of an ASCII byte (`@` a letter when `at`).
fn plain_catcode(b: u8, at: bool) -> u8 {
    match b {
        b'\\' => 0,
        b'{' => 1,
        b'}' => 2,
        b'$' => 3,
        b'&' => 4,
        13 => 5,
        b'#' => 6,
        b'^' => 7,
        b'_' => 8,
        0 => 9,
        b' ' | b'\t' => 10,
        b'~' => 13,
        b'%' => 14,
        127 => 15,
        b'@' if at => 11,
        _ if b.is_ascii_alphabetic() => 11,
        _ => 12,
    }
}

// ---------------------------------------------------------------------
// At the node.

static OFF: AtomicBool = AtomicBool::new(false);

/// Turns the hints off (every node may define anything, as without
/// them) or on again, for comparisons; `PHITEX_HINTS_OFF=1` does the
/// same for a process.
pub fn set_hints_enabled(on: bool) {
    OFF.store(!on, Ordering::Relaxed);
}

/// Whether the node's text says its command opens and closes no scope.
pub(crate) fn effect_none(reg: &Registry, v: &Values, args: &[ValueId]) -> bool {
    if OFF.load(Ordering::Relaxed) {
        return false;
    }
    hint_of_args(reg, v, args)
        .bound
        .as_ref()
        .is_some_and(|b| b.no_scope)
}

/// The fields the hint rests on, which the node reads first: the
/// meanings of the names its text calls (`cs:name`, `act:code`) and the
/// catcodes its bytes were read with (`cat:c`).
pub(crate) fn reads(reg: &Registry, v: &Values, args: &[ValueId]) -> Option<Vec<Field>> {
    take_prepared(reg, args, |p| match std::mem::take(&mut p.reads) {
        Kept::Ready(r) => Some(r),
        Kept::Taken => None,
    })
    .unwrap_or_else(|| reads_now(reg, v, args))
}

/// [`reads`], answered now.
fn reads_now(reg: &Registry, v: &Values, args: &[ValueId]) -> Option<Vec<Field>> {
    if OFF.load(Ordering::Relaxed) {
        return None;
    }
    let h = hint_of_args(reg, v, args);
    let b = h.bound.as_ref()?;
    let mut out: Vec<Field> = b.means.iter().map(|(n, _)| v.field(n)).collect();
    // (a record's cells are read from the record)
    let mut recs = [false; crate::fields::NREC];
    recs[crate::fields::B_CAT] = !b.cats.is_empty();
    for n in &b.reads {
        match crate::fields::record_of_name(n) {
            Some(r) => recs[r] = true,
            None => out.push(v.field(n)),
        }
    }
    for (r, &on) in recs.iter().enumerate() {
        if on {
            out.push(v.field(crate::fields::RECORDS[r]));
        }
    }
    (!out.is_empty()).then_some(out)
}

/// The engine's question, answered from the text (made ahead when the
/// node's push was [`prepare`]d).
pub(crate) fn may_define(reg: &Registry, v: &Values, args: &[ValueId]) -> MayDefine {
    take_prepared(reg, args, |p| p.may.take()).unwrap_or_else(|| may_define_now(reg, v, args))
}

/// [`may_define`], answered now.
fn may_define_now(reg: &Registry, v: &Values, args: &[ValueId]) -> MayDefine {
    if OFF.load(Ordering::Relaxed) {
        return MayDefine::Any;
    }
    // (an answer is the hint's alone: made once per hint string)
    let hv = hint_value(v, args).filter(|_| !crate::exec::obs_flag!("PHITEX_HINTS"));
    if let Some(hv) = hv
        && let Some(m) = reg.answers.locked().get(&hv.raw())
    {
        return m.clone();
    }
    let m = may_define_answer(reg, v, args);
    if let Some(hv) = hv {
        let mut a = reg.answers.locked();
        if a.len() > 1 << 16 {
            a.clear();
        }
        a.insert(hv.raw(), m.clone());
    }
    m
}

/// [`may_define_now`], made.
fn may_define_answer(reg: &Registry, v: &Values, args: &[ValueId]) -> MayDefine {
    let h = hint_of_args(reg, v, args);
    if crate::exec::obs_flag!("PHITEX_HINTS") {
        let answer = match &h.bound {
            None => "Any".to_owned(),
            Some(b) => cat(&[
                "Prefixes(",
                &b.state
                    .as_ref()
                    .map_or_else(|| "every head".to_owned(), |st| st.join(" ")),
                " + [",
                &b.names.join(", "),
                "] + [",
                &b.prefixes.join(", "),
                "])",
            ]),
        };
        eprintln!("hint {:?}: {answer} ({})", h.excerpt, h.cause);
    }
    // (a package load's record: exactly the fields it defined, never a
    // prefix of them, `crate::pkgrec`)
    if is_record(&h)
        && let Some(b) = &h.bound
    {
        let mut ns = b.names.clone();
        for r in crate::fields::RECORDS {
            if !ns.iter().any(|n| n == r)
                && ns.iter().any(|n| {
                    crate::fields::record_of_name(n).is_some_and(|x| crate::fields::RECORDS[x] == r)
                })
            {
                ns.push(r.to_owned());
            }
        }
        return MayDefine::Names(ns.iter().map(|n| v.field(n)).collect());
    }
    h.may_define()
}

/// The cause of a package load's hint taken from its record
/// ([`crate::pkgrec`]).
pub(crate) const RECORD_CAUSE: &str = "package record";

/// The cause of a never-seen package load's optimistic hint.
pub(crate) const OPTIMISTIC_CAUSE: &str = "package not recorded: optimistic";

/// Whether the hint is a package load's record.
pub(crate) fn is_record(h: &Hint) -> bool {
    h.bound.is_some() && h.cause == RECORD_CAUSE
}

/// A package load's hint from its record: it defines exactly `writes`
/// (no state head besides them), reads `reads`, and, when its last run
/// said so, opens no scope and takes no sibling.
pub(crate) fn record_hint(
    writes: &[String],
    reads: &[String],
    no_scope: bool,
    claim0: bool,
) -> String {
    Hint {
        bound: Some(Bound {
            names: writes.to_vec(),
            state: Some(Vec::new()),
            no_scope,
            claim: claim0.then_some(0),
            reads: reads.to_vec(),
            ..Bound::default()
        }),
        cause: RECORD_CAUSE.to_owned(),
        excerpt: String::new(),
        guess: None,
    }
    .encode()
}

/// A never-seen package load's hint: unbounded, guessed to define no
/// control sequence, so a later load reading one runs on the state as it
/// stands (under speculation), refuted by the engine where the load did
/// define it.
pub(crate) fn optimistic_hint() -> String {
    Hint {
        bound: None,
        cause: OPTIMISTIC_CAUSE.to_owned(),
        excerpt: String::new(),
        guess: Some(Bound::default()),
    }
    .encode()
}

/// The accumulators the nodes count into.
pub(crate) const COUNTERS: [&str; 9] = [
    "hints:bounded",
    "hints:any",
    "hints:violations",
    "hints:demoted",
    // (nodes whose text is a token list, and those reading source text)
    "frames:tokens",
    "frames:text",
    // (nodes whose own slice is a paragraph region, `slices_of`)
    "frames:pars",
    // (paragraph-region slices another node's run took, pulled)
    "frames:pars_taken",
    // (groups a run could not leave to the engine as a scope for a
    // field at both levels: none since the engine's two-entry form)
    "frames:scope_both",
];

/// Classes whose state row still let a run define outside it in the
/// suite: they answer `Any` until each is fixed (with its section) and
/// promoted again.
const DEMOTED: &[&str] = &[
    // (none: each class above was completed from tex.web and promoted)
];

fn demoted(class: &str, excerpt: &str) -> Hint {
    Hint::any(cat(&["demoted: ", class]), excerpt)
}

/// Whether the hint answers `Any` because its class is demoted.
pub(crate) fn is_demoted(h: &Hint) -> bool {
    h.bound.is_none() && h.cause.starts_with("demoted: ")
}

/// Checks the hint's assumptions against the meanings and catcodes in
/// force where the node runs: the violation, if any.
pub(crate) fn verify(r: &mut Run<'_, '_>, h: &Hint) -> R<Option<String>> {
    let Some(b) = &h.bound else {
        return Ok(None);
    };
    for &(byte, cat_code) in b.cats.iter() {
        let got = r.cat(u32::from(byte))?;
        if got != cat_code && (cat_code != NOT_LETTER || got == 11) {
            return Ok(Some(cat(&[
                "catcode of '",
                char::from(byte).encode_utf8(&mut [0; 4]),
                "' is ",
                &got.to_string(),
                ", the hint read it as ",
                &cat_code.to_string(),
                " (",
                &h.excerpt,
                ")",
            ])));
        }
    }
    for (name, class) in &b.means {
        let x = r.get(name)?;
        let m = r.decode_mean(x);
        let ok = match class {
            0 => matches!(m, Mean::Prim(..)),
            _ => !matches!(m, Mean::Undefined),
        };
        if !ok {
            return Ok(Some(cat(&[
                name,
                " is ",
                mean_kind(&m),
                " where the hint assumed ",
                if *class == 0 {
                    "a primitive"
                } else {
                    "a definition"
                },
                " (",
                &h.excerpt,
                ")",
            ])));
        }
    }
    Ok(None)
}

fn mean_kind(m: &Mean) -> &'static str {
    match m {
        Mean::Prim(..) => "a primitive",
        Mean::Macro(_) => "a macro",
        Mean::Char(..) => "a character",
        Mean::Undefined => "undefined",
        Mean::Font(_) => "a font",
        Mean::CharDef(_) | Mean::MathCharDef(_) => "a chardef",
        Mean::Reg(..) => "a register",
        Mean::NotExpanded => "not expanded",
    }
}

/// `PHITEX_HINTS=2`: the node's definitions, for learning the state
/// each command sets.
pub(crate) fn debug_defs(r: &mut Run<'_, '_>, h: &Hint) {
    // (`PHITEX_HINTS=3`: each node's published fields, against its bound)
    if crate::exec::obs_str!("PHITEX_HINTS") == Some("3") {
        let fs: Vec<String> = r
            .published_fields()
            .into_iter()
            .map(|f| crate::val::field_name(r.v, f))
            .collect();
        let ex: String = h.excerpt.chars().take(40).collect();
        eprintln!(
            "pubs {ex:?} void {} state {:?}: {}",
            r.hint_void,
            h.bound.as_ref().map(|b| b.state.clone()),
            fs.join(",")
        );
    }
    if crate::exec::obs_str!("PHITEX_HINTS") == Some("2") {
        let mut fs: Vec<String> = r
            .def_fields()
            .into_iter()
            .filter_map(|f| match &*r.v.get(f.0) {
                Value::Str(s) if !is_cs_field(s) => {
                    Some(s.split(':').next().unwrap_or(s).to_owned())
                }
                Value::Str(_) => Some("<cs>".to_owned()),
                _ => None,
            })
            .collect();
        fs.sort();
        fs.dedup();
        let first = h
            .excerpt
            .split([' ', '{', '#', '='])
            .next()
            .unwrap_or("")
            .to_owned();
        eprintln!("defs {first}: {}", fs.join(","));
    }
}

/// Counts the node into accumulator `k` of [`COUNTERS`].
pub(crate) fn count(r: &mut Run<'_, '_>, k: usize) {
    let one = r.v.int(1);
    r.contribute(COUNTERS[k], one);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hint_of(text: &str, at: usize) -> Hint {
        let t = analyse(text, FileKind::Document, no_defs());
        t.at(at)
    }

    #[test]
    fn def_is_bounded() {
        let h = hint_of("\\def\\foo#1{x#1y}\n\\foo{a}\n", 0);
        let b = h.bound.expect("bounded");
        assert_eq!(b.names, vec!["cs:foo".to_owned()]);
        assert!(b.cats.contains(&(b'\\', 0)));
        assert!(b.cats.contains(&(b'{', 1)));
        assert!(b.means.contains(&("cs:def".to_owned(), 1)));
        assert_eq!(b.state.as_deref(), Some(&[][..]));
        // the call: its body defines nothing
        let h = hint_of("\\def\\foo#1{x#1y}\n\\foo{a}\n", 17);
        let b = h.bound.expect("bounded");
        assert!(b.names.is_empty(), "{b:?}");
        assert!(b.means.contains(&("cs:foo".to_owned(), 1)));
    }

    #[test]
    fn unknown_macro_is_any() {
        let h = hint_of("\\bar\n", 0);
        assert!(h.bound.is_none(), "{h:?}");
    }

    #[test]
    fn global_prefix_defines() {
        let h = hint_of("\\global\\def\\x{}\n", 0);
        let b = h.bound.expect("bounded");
        assert_eq!(b.names, vec!["cs:x".to_owned()]);
    }

    #[test]
    fn round_trip() {
        let h = hint_of("\\let\\a=\\b\n", 0);
        let d = Hint::decode(&h.encode());
        assert_eq!(d.bound, h.bound);
        let a = Hint::any("why", "text");
        assert!(Hint::decode(&a.encode()).bound.is_none());
    }

    #[test]
    fn active_char_is_assumed() {
        let h = hint_of("a~b\n", 0);
        let b = h.bound.expect("bounded: `~` defines nothing in the text");
        assert!(b.means.contains(&("act:126".to_owned(), 1)), "{b:?}");
        assert!(b.cats.contains(&(b'~', 13)));
    }

    #[test]
    fn control_space_after_a_macro_is_a_region() {
        let t = "\\def\\hello{Hello}\n\\hello\\ world.\n\\bye\n";
        // (under the plain prelude's definitions, as a build reads it)
        let fx = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../oracle/fixtures/dvi/plaintext.tex");
        let plain = std::fs::read_to_string(fx).expect("plaintext.tex");
        let mut d = pass::Definitions::new();
        d.harvest(&plain, kind_of(&plain, FileKind::Package), 0);
        let defs: Defs = Arc::new(d);
        let an = Analysis::new(t, FileKind::Document, defs);
        let (sl, pars) = slices_of(t, &an, FileKind::Document);
        eprintln!("{sl:?} {pars:?}");
        let h = t.find("world").expect("world");
        let k = sl
            .iter()
            .position(|x| x.0 <= h && h < x.1)
            .expect("a slice");
        assert!(pars[k], "{sl:?} {pars:?}");
    }

    #[test]
    fn routine_advancing_the_page_number_defines_that_register() {
        // (plain's routine advances `\\pageno`, `\\countdef`'d to 0: the
        // finisher may define count:0, never another count register)
        let fx = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../oracle/fixtures/dvi/plaintext.tex");
        let plain = std::fs::read_to_string(fx).expect("plaintext.tex");
        let mut d = pass::Definitions::new();
        d.harvest(&plain, kind_of(&plain, FileKind::Package), 0);
        let defs: Defs = Arc::new(d);
        let h = routine_hint("\\plainoutput", &defs);
        eprintln!(
            "{} {:?}",
            h.cause,
            h.bound
                .as_ref()
                .or(h.guess.as_ref())
                .map(|b| (&b.names, &b.prefixes, &b.state))
        );
        assert!(h.may_define_field("count:0"));
        assert!(!h.may_define_field("count:24"));
    }

    #[test]
    fn bytes_in_decimal_as_formatted() {
        for x in 0..=255u8 {
            let mut o = String::new();
            push_u8(&mut o, x);
            assert_eq!(o, x.to_string());
        }
    }

    #[test]
    fn initex_texts_as_before() {
        // (the two searches it replaces)
        fn before(text: &str) -> bool {
            let brace = text.find('{');
            let line = text
                .find("\\catcode`\\{")
                .or_else(|| text.find("\\catcode123"));
            match (line, brace) {
                (Some(l), Some(b)) => l < b,
                (Some(_), None) => true,
                _ => false,
            }
        }
        for t in [
            "",
            "plain text",
            "{\\catcode`\\{=1",
            "\\catcode`\\{=1 {",
            "\\catcode123=1 {",
            "{ \\catcode123=1",
            "\\catcode123=1 { \\catcode`\\{=1",
            "\\catcode`\\}=2 \\catcode123=1",
            "\\catcode\\catcode123 x",
            "a \\catcode`\\{ b \\catcode123 {",
        ] {
            assert_eq!(looks_initex(t), before(t), "{t:?}");
        }
    }

    #[test]
    fn reset_pending_across_slices() {
        // (`\\parshape` set in one slice, the paragraph ended in the
        // next: that one's `normal_paragraph` resets it, §1070; the one
        // after finds it reset already; the paragraph's first character
        // begins a slice of its own, a paragraph start being a cut)
        let t = "\\parshape 1 0pt 100pt text\nmore text\\par\nthird\\par\n";
        let text = analyse(t, FileKind::Document, no_defs());
        let slice = |needle: &str| -> usize {
            text.slices
                .iter()
                .position(|&(s, e)| t[s..e].contains(needle))
                .expect("a slice")
        };
        assert_ne!(
            slice("parshape"),
            slice("more"),
            "set in one slice, ended in the next"
        );
        let state = |needle: &str| -> Vec<&'static str> {
            let k = slice(needle);
            eprintln!("{needle}: {:?}", &t[text.slices[k].0..text.slices[k].1]);
            Hint::decode(&text.slice_hints[k])
                .bound
                .and_then(|b| b.state)
                .expect("a state row")
        };
        assert!(state("more").contains(&"parshape"));
        assert!(!state("third").contains(&"parshape"));
    }

    #[test]
    fn paragraph_and_conditional_classes() {
        let t = "\\par\nText here \\ifnum1=1 \\fi\n\\afterassignment\\relax\\def\\y{}\n";
        let text = analyse(t, FileKind::Document, no_defs());
        for off in [0, 5, 10, 15] {
            let h = text.at(off);
            eprintln!(
                "{off}: {} {:?}",
                h.cause,
                h.bound.as_ref().map(|b| &b.state)
            );
        }
        let st = |off: usize| text.at(off).bound.expect("bounded").state;
        // (`\\par` is bounded: a paragraph's end, §1094-§1096; its
        // `normal_paragraph` resets only what a command since set, §1070)
        let par = st(0).expect("a state row");
        assert!(par.contains(&"pg:") && !par.contains(&"parshape"));
        // the run after it: `\\prevgraf` if a command after it in its
        // text ends the paragraph, and `\\afterassignment`'s row (the
        // region runs on to its end); no dead head (`par:`, `if:`,
        // `lopen:`, `page:`) is ever declared
        assert_eq!(st(10).as_deref(), Some(&["pg:", "aftertoken"][..]));
        for off in [0, 5, 10] {
            let v = st(off).expect("some");
            assert!(
                !v.iter()
                    .any(|h| ["par:", "if:", "lopen:", "page:"].contains(h)),
                "{off}: {v:?}"
            );
        }
        let d = text.at(t.find("\\def").unwrap()).bound.expect("bounded");
        assert!(d.state.expect("some").contains(&"aftertoken"));
        assert_eq!(d.names, vec!["cs:y".to_owned()]);
    }

    #[test]
    fn state_prefixes_cover_the_engine_fields() {
        let md = Hint {
            bound: Some(Bound::default()),
            ..Hint::default()
        }
        .may_define();
        let MayDefine::Prefixes(ps) = md else {
            panic!("prefixes")
        };
        for f in ["pg:1", "aftertoken", "cat:64", "count:10", "fontinfo:2"] {
            assert!(ps.iter().any(|p| f.starts_with(p.as_str())), "{f}");
        }
        for f in ["cs:foo", "act:126"] {
            assert!(!ps.iter().any(|p| f.starts_with(p.as_str())), "{f}");
        }
    }
}
