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

use phitex_defuse::FxHashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use phitex_defuse::{Field, MayDefine, Value, ValueId, Values};
use phitex_source::pass::{self, Atom, AtomKind, Def, DefSet, Definitions, FileKind, Pass, Scope};

use crate::run::{R, Run};
use crate::val::{Mean, Tok, int_of, list_of, str_of};

/// The definitions the hints may bind calls to besides the text's own:
/// the format's (shared by every text of a build).
pub(crate) type Defs = Arc<pass::Definitions>;

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
    pub cats: Vec<(u8, u8)>,
    /// Meaning fields (`cs:name`, `act:code`) and the class assumed: 0
    /// a primitive, 1 anything defined.
    pub means: Vec<(String, u8)>,
    /// The state fields (prefixes) the command's class may set, or
    /// `None`: every state head ([`state_prefixes`]).
    pub state: Option<Vec<String>>,
    /// The command opens and closes no scope (its effect is `None`).
    pub no_scope: bool,
    /// For a slice: the following siblings its run takes, when the text
    /// says so exactly (`Some(0)`: none); `None` when only the run knows.
    pub claim: Option<usize>,
}

/// The state fields (heads: a family's prefix with its colon, or a
/// cell's name) a command of this name may set besides control
/// sequences, from tex.web; `None`: a command whose state is not
/// listed here (every head). The alignment state (`align:`), `\\prevgraf`
/// (`pg:`) and `aftertoken` come from the context, not from the command ([`Analysis::cs_hint`]).
#[allow(clippy::too_many_lines, clippy::match_same_arms)]
fn class_state(name: &str) -> Option<&'static [&'static str]> {
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
        // the same node is packaged there and sets \\\\badness, §1086, §649)
        "hbox" | "vbox" | "vtop" | "vcenter" => &["lvlfix", "badness", "markclasses"],
        "setbox" | "vsplit" => &["box:", "lvlfix", "badness", "markclasses"],
        // conditionals (§498-§510)
        "if" | "ifx" | "ifnum" | "ifdim" | "ifodd" | "ifvmode" | "ifhmode" | "ifmmode"
        | "ifinner" | "ifvoid" | "ifhbox" | "ifvbox" | "ifcat" | "ifcase" | "iftrue"
        | "iffalse" | "ifdefined" | "ifcsname" | "iffontchar" | "unless" | "else" | "fi" | "or"
        | "ifpdfprimitive" | "ifpdfabsnum" | "ifpdfabsdim" | "ifincsname" | "ifeof" => &[],
        // fonts (§1257 new_font, §1234)
        "font" => &[
            "fd:",
            "fdn:",
            "fontcount",
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
                let f: &'static str = Box::leak(format!("param:{}", p.0).into_boxed_str());
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
    crate::prims::PRIMS
        .iter()
        .any(|p| p.0 == name && matches!(p.1, 72..=76))
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

/// Adds heads to a state set (`None`: every head already).
fn add_heads(st: &mut Option<Vec<String>>, heads: Option<&[&str]>) {
    match (st.as_mut(), heads) {
        (Some(v), Some(hs)) => {
            for h in hs {
                if std::env::var("PHITEX_HEAD_TRACE").is_ok_and(|w| *h == w) {
                    eprintln!("HEADTRACE {h}\n{}", std::backtrace::Backtrace::force_capture());
                }
                if !v.iter().any(|x| x == h) {
                    v.push((*h).to_owned());
                }
            }
        }
        (Some(_), None) => *st = None,
        (None, _) => {}
    }
}

fn add_head(st: &mut Option<Vec<String>>, h: &str) {
    add_heads(st, Some(&[h]));
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
                v.push(format!("c{}", c as char));
            }
            if c != b'c' {
                v.push(format!("a{}", c as char));
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
                    Some(st) => st.clone(),
                    None => state_prefixes().to_vec(),
                };
                ps.extend(b.names.iter().cloned());
                ps.extend(b.prefixes.iter().cloned());
                MayDefine::Prefixes(ps)
            }
        }
    }

    /// `A` or `B`, the cause, the excerpt, then the bound's parts;
    /// records by `\x1e`, items by `\x1f`, pairs by `\x1d`.
    fn encode(&self) -> String {
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

    fn decode(s: &str) -> Hint {
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
        if crate::exec::env_flag!("PHITEX_NO_GUESS") {
            return true;
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
            Some(st) => st.iter().any(|h| f.starts_with(h.as_str())),
        }
    }
}

/// A bound's fields after a hint's excerpt (`\x1e` before each).
fn encode_bound(b: &Bound, o: &mut String) {
    o.push('\x1e');
    o.push_str(&b.names.join("\x1f"));
    o.push('\x1e');
    o.push_str(&b.prefixes.join("\x1f"));
    o.push('\x1e');
    let cats: Vec<String> = b.cats.iter().map(|(c, k)| format!("{c}\x1d{k}")).collect();
    o.push_str(&cats.join("\x1f"));
    o.push('\x1e');
    let means: Vec<String> = b.means.iter().map(|(n, k)| format!("{n}\x1d{k}")).collect();
    o.push_str(&means.join("\x1f"));
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
        st => Some(items(st)),
    };
    let no_scope = it.next() == Some("N");
    let claim = it.next().and_then(|c| c.parse().ok());
    Bound {
        names,
        prefixes,
        cats,
        means,
        state,
        no_scope,
        claim,
    }
}

fn short(s: &str) -> String {
    let t: String = s.chars().take(40).collect();
    t.replace('\n', "\\n")
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
    Some(format!("cs:{name}"))
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
            Some(st) => st.iter().any(|x| name.starts_with(x.as_str())),
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
    /// Byte starts of the atoms, and the encoded hint of the command
    /// beginning there.
    starts: Vec<usize>,
    hints: Vec<Arc<Hint>>,
    /// Per atom: an operand an earlier command reads (never run on its
    /// own), or a character after the first of its run.
    skip: Vec<bool>,
    /// Per slice: whether a paragraph starts in it (a region: its run
    /// begins and ends in the vertical list).
    pars: Vec<bool>,
    /// Per slice.
    slice_hints: Vec<Arc<str>>,
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
    fn at_until(&self, off: usize, end: usize) -> Hint {
        let k = self.starts.partition_point(|&s| s < off);
        let mut acc: Option<Bound> = None;
        let mut first: Option<Hint> = None;
        // (the first unbounded command, and whether every unbounded one
        // has a guess: the span's guess is then the union)
        let mut any: Option<Hint> = None;
        let mut guessed = true;
        let mut a = k;
        while let Some(&s) = self.starts.get(a)
            && s < end
        {
            a += 1;
            if self.skip[a - 1] {
                continue;
            }
            let h = &*self.hints[a - 1];
            if first.is_none() {
                first = Some(h.clone());
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
            match &mut acc {
                Some(acc) => union(acc, part),
                None => acc = Some(part.clone()),
            }
        }
        if let Some(mut h) = any {
            h.guess = if guessed { acc } else { None };
            return h;
        }
        match (first, acc) {
            (Some(mut h), Some(b)) => {
                h.bound = Some(b);
                h
            }
            (Some(h), None) => h,
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
#[allow(clippy::too_many_lines)]
pub(crate) fn slices_of(text: &str, p: &Pass, kind: FileKind) -> (Vec<(usize, usize)>, Vec<bool>) {
    if kind != FileKind::Document {
        return if text.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            (vec![(0, text.len())], vec![true])
        };
    }
    let n = p.atoms.len();
    // brace depth before each atom
    let mut depth = Vec::with_capacity(n);
    let mut d: usize = 0;
    for at in &p.atoms {
        if at.kind == AtomKind::Close {
            d = d.saturating_sub(1);
        }
        depth.push(d);
        if at.kind == AtomKind::Open {
            d += 1;
        }
    }
    // atoms an earlier item claims (operands: never a slice's start)
    let mut claimed = vec![false; n];
    for (i, it) in p.items.iter().enumerate() {
        // (a value whose last token is a command the pass cannot read
        // through, `\count1=\input inc-num`: the text up to the next
        // command is the value's too, §440)
        let end = match &it.hints.claim {
            pass::Claim::Pending { cause, .. }
                if matches!(*cause, "command in value position" | "value is a macro") =>
            {
                p.items.get(i + 1).map_or(it.end, |x| x.atom).max(it.end)
            }
            _ => it.end,
        };
        for c in claimed.iter_mut().take(end.min(n)).skip(it.atom + 1) {
            *c = true;
        }
    }
    // (that extension runs in the command's own run: `\\vskip 1in
    // \\centerline{…}` expands the macro while it looks for `plus`,
    // §1094, and runs what it leaves; one slice)
    let mut extended = vec![false; n];
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
            for c in extended
                .iter_mut()
                .take(end.min(n))
                .skip(it.end.max(it.atom + 1))
            {
                *c = true;
            }
        }
    }
    // the operands of a primitive (`\sfcode`\)=0`): never a slice's
    // start, so a command and what it reads are one node (a macro's
    // operands are the pass's guess and do not count)
    let mut operand = vec![false; n];
    for it in &p.items {
        if is_prim(&it.name) {
            for c in operand.iter_mut().take(it.end.min(n)).skip(it.atom + 1) {
                *c = true;
            }
        }
    }
    for (o, &x) in operand.iter_mut().zip(&extended) {
        *o |= x;
    }
    // the conditional nesting before each atom, by the text's items
    // (`\if…` +1, `\fi` -1; an item an earlier one consumes not counted)
    let mut ifd = vec![0i64; n + 1];
    {
        let mut consumed = 0;
        let mut cur = 0i64;
        let mut k = 0;
        let mut deltas: Vec<(usize, i64)> = Vec::new();
        for it in &p.items {
            if it.atom >= consumed {
                if CONDITIONALS.contains(&it.name.as_str()) {
                    deltas.push((it.atom, 1));
                } else if it.name == "fi" {
                    deltas.push((it.atom, -1));
                }
            }
            if it.consumes {
                consumed = consumed.max(it.end);
            }
        }
        for (a, x) in ifd.iter_mut().enumerate() {
            while k < deltas.len() && deltas[k].0 < a {
                cur = (cur + deltas[k].1).max(0);
                k += 1;
            }
            *x = cur;
        }
    }
    // inside a brace group of the text opened at depth 0: the slice goes
    // on to its end (a brace group of the text is never split)
    let mut inside = vec![false; n];
    for (k, a) in p.atoms.iter().enumerate() {
        // (a brace knows its mate's atom index; `p.braces` indexes tokens)
        if a.kind == AtomKind::Open && a.mate != usize::MAX && a.mate > k && depth[k] == 0 {
            for x in inside.iter_mut().take(a.mate + 1).skip(k + 1) {
                *x = true;
            }
        }
    }
    // (a `\\begingroup`...`\\endgroup` pair or an environment is no
    // bar to a cut: a paragraph end inside it at brace depth 0 of the
    // text is one, the region that leaves the group open leaves it to the
    // engine as a scope and the one that closes it closes that scope,
    // `Run::run_all`, `Run::close_group`)
    // a control sequence at depth 0 and its `[...]`/`{...}` arguments
    // (with the spaces between): one slice; a loading command's are
    // never cut and end a paragraph before it
    let mut loaders: Vec<usize> = Vec::new();
    for k in 0..n {
        let a = &p.atoms[k];
        if a.kind != AtomKind::Cs || depth[k] != 0 || ifd[k] != 0 || inside[k] || claimed[k] {
            continue;
        }
        let name = a.name(text);
        let loader = matches!(
            name,
            "documentclass" | "usepackage" | "RequirePackage" | "LoadClass" | "input" | "include"
        );
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
                while q < n && !(p.atoms[q].kind == AtomKind::Char(']') && depth[q] == depth[j]) {
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
                && claimed[j]
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
        }
        for x in inside.iter_mut().take(last + 1).skip(k + 1) {
            *x = true;
        }
    }
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
                && depth.get(it.atom) == Some(&0)
                && it.hints.implicit == pass::Implicit::EndsParagraph
                && ifd[it.atom] == 0
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
            if depth.get(m.atom) == Some(&0)
                && m.hints.implicit == pass::Implicit::EndsParagraph
                && ifd[m.atom] == 0
            {
                ends.push(m.atom + 1);
            }
        }
        // (a loading command is vertical: the paragraph ends before it)
        ends.extend(loaders.iter().copied());
        ends.sort_unstable();
        ends.dedup();
    }
    let mut par_at = vec![false; n];
    {
        let mut k = 0;
        while k < n {
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
                            !claimed.get(j).copied().unwrap_or(true)
                                && !inside.get(j).copied().unwrap_or(true)
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
            if horizontal && depth[k] == 0 && ifd[k] == 0 && !inside[k] && !operand[k] {
                // the first atom after the paragraph's end, at depth 0
                // and outside every conditional
                let mut e = ends.partition_point(|&x| x <= k);
                let mut end = ends.get(e).copied().unwrap_or(n);
                while end < n && (depth[end] != 0 || ifd[end] != 0) {
                    end += 1;
                    e += 1;
                    if let Some(&x) = ends.get(e) {
                        end = end.max(x);
                    }
                }
                par_at[k] = true;
                for x in inside.iter_mut().take(end.min(n)).skip(k + 1) {
                    *x = true;
                }
                k = end.max(k + 1);
                continue;
            }
            k += 1;
        }
    }
    if crate::exec::env_flag!("PHITEX_SLICES_TRACE") {
        for (k, a) in p.atoms.iter().enumerate() {
            eprintln!(
                "slices: {k} {:?} {:?} depth {} ifd {} inside {} claimed {} operand {} par {}",
                a.kind,
                &text[a.start..a.end],
                depth[k],
                ifd[k],
                inside[k],
                claimed[k],
                operand[k],
                par_at[k]
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
    for (k, a) in p.atoms.iter().enumerate() {
        let ch = matches!(a.kind, AtomKind::Char(_) | AtomKind::Verb);
        let inside_if =
            ifd[k] > 0 && (a.kind != AtomKind::Cs || !CONDITIONALS.contains(&a.name(text)));
        // (an operand a primitive reads never starts a slice; a macro's
        // operand range is the pass's guess and does not cut)
        if !(inside_if || inside[k] || operand[k] || (ch && prev_char)) {
            starts.push(a.start);
        }
        prev_char = ch;
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
            let has_par = p.atoms.iter().enumerate().any(|(k, a)| {
                a.start >= s
                    && a.start < e
                    && (par_at[k]
                        || (depth[k] == 0
                            && ifd[k] == 0
                            && !operand[k]
                            && match a.kind {
                                AtomKind::Char(_) | AtomKind::Verb => true,
                                AtomKind::Cs => starts_par(a.name(text)),
                                _ => false,
                            }))
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
    let lo = an.p.atoms.partition_point(|a| a.start < s);
    let hi = an.p.atoms.partition_point(|a| a.start < e);
    for a in &an.p.atoms[lo..hi.max(lo)] {
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
                        if crate::exec::env_flag!("PHITEX_SLICES_TRACE") {
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
                        for c in an.defset(n).calls {
                            if !calls.contains(&c) {
                                calls.push(c);
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
fn union(a: &mut Bound, b: &Bound) {
    for p in &b.prefixes {
        if !a.prefixes.contains(p) {
            a.prefixes.push(p.clone());
        }
    }
    for c in &b.cats {
        if !a.cats.contains(c) {
            a.cats.push(*c);
        }
    }
    for m in &b.means {
        // (a meaning an earlier command of the span defines is the
        // run's own, not assumed of the state before it)
        if !a.means.iter().any(|x| x.0 == m.0) && !a.names.contains(&m.0) {
            a.means.push(m.clone());
        }
    }
    for n in &b.names {
        if !a.names.contains(n) {
            a.names.push(n.clone());
        }
    }
    match (&mut a.state, &b.state) {
        (Some(sa), Some(sb)) => {
            for h in sb {
                if !sa.contains(h) {
                    sa.push(h.clone());
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
struct Analysis<'t> {
    text: &'t str,
    p: Pass,
    defs: Definitions,
    /// Item index by atom.
    item_of: FxHashMap<usize, usize>,
    /// Atoms an earlier item claims (operands: never run on their own).
    claimed: Vec<bool>,
    /// Per atom: the index of its regime in the pass's regimes.
    reg: Vec<usize>,
    /// Atoms textually inside an alignment's braces (`\\halign{`…`}`).
    in_align: Vec<bool>,
    /// Atoms inside a brace group of the text: a font they select is
    /// the group's, restored at its `}` (§1217), never a definition of
    /// the node.
    inner: Vec<bool>,
    memo: std::cell::RefCell<FxHashMap<String, DefSet>>,
    /// The names whose state [`Analysis::defined_state`] is reading.
    visiting: std::cell::RefCell<Vec<String>>,
    /// The format's definitions, behind the text's own.
    prelude: Defs,
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
            | "tex_indent:D"
            | "tex_noindent:D"
    )
}

impl Analysis<'_> {
    /// Whether the items from atom `k0` to byte `e` all end before `e`
    /// by a known extent, and none ends the job.
    fn slice_claims_nothing(&self, k0: usize, e: usize) -> bool {
        let atoms = self.atoms();
        let k1 = atoms.partition_point(|x| x.start < e);
        let i0 = self.p.items.partition_point(|it| it.atom < k0);
        // (the slice must end in a paragraph end of its own text, a blank
        // line or `\\par`: otherwise a paragraph begun here reads on into
        // the siblings until something ends it, §1094, and a command at
        // its end that scans for a keyword or unit, `\\medskip`'s
        // `\\vskip`, reads the next sibling's first token, §407)
        {
            let last = (k0..k1).rev().find(|&a| atoms[a].kind != AtomKind::Space);
            if !last.is_some_and(|a| self.ends_par_at(a)) {
                return false;
            }
        }
        for it in self.p.items[i0..].iter().take_while(|it| it.atom < k1) {
            if it.end > k1 || !matches!(it.hints.claim, pass::Claim::Known(_)) {
                return false;
            }
            if let Some(at) = atoms.get(it.atom)
                && at.kind == AtomKind::Cs
            {
                let n = at.name(self.text);
                if matches!(n, "end" | "dump" | "bye")
                    || self
                        .defset(n)
                        .calls
                        .iter()
                        .any(|c| matches!(c.as_str(), "end" | "dump"))
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

    fn new(text: &str, kind: FileKind, prelude: Defs) -> Analysis<'_> {
        let mut defs = Definitions::new();
        defs.harvest(text, kind, 0);
        let scope = Scope {
            document: &defs,
            file: 0,
            prelude: &prelude,
        };
        let p = pass::analyze(text, kind, scope);
        let item_of = p
            .items
            .iter()
            .enumerate()
            .map(|(i, it)| (it.atom, i))
            .collect();
        let mut claimed = vec![false; p.atoms.len()];
        for it in &p.items {
            // (a scope opener, `\\hbox to 3pt{`, claims its operands up
            // to its brace; the list inside runs, §1083, §1086)
            let end = match (&it.hints.scope, &it.hints.claim) {
                (pass::ScopeHint::Open(_), pass::Claim::Known(b)) => {
                    let to = p.atoms.get(it.atom).map_or(0, |a| a.end) + b;
                    p.atoms.partition_point(|a| a.start < to).min(it.end)
                }
                _ => it.end,
            };
            for c in claimed
                .iter_mut()
                .take(end.min(p.atoms.len()))
                .skip(it.atom + 1)
            {
                *c = true;
            }
        }
        let in_align = align_atoms(text, &p.atoms);
        let mut inner = Vec::with_capacity(p.atoms.len());
        let mut d = 0usize;
        for a in &p.atoms {
            if a.kind == AtomKind::Close {
                d = d.saturating_sub(1);
            }
            inner.push(d > 0);
            if a.kind == AtomKind::Open {
                d += 1;
            }
        }
        let reg: Vec<usize> = {
            let mut k = 0;
            p.atoms
                .iter()
                .map(|a| {
                    while k + 1 < p.regimes.len() && p.regimes[k + 1].0 <= a.tok {
                        k += 1;
                    }
                    k.min(p.regimes.len().saturating_sub(1))
                })
                .collect()
        };
        Analysis {
            text,
            p,
            defs,
            item_of,
            claimed,
            reg,
            in_align,
            inner,
            memo: std::cell::RefCell::new(FxHashMap::default()),
            visiting: std::cell::RefCell::new(Vec::new()),
            prelude,
        }
    }

    fn atoms(&self) -> &[Atom] {
        &self.p.atoms
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
            return Some(Some(format!("param:{n}")));
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
    fn defset(&self, name: &str) -> DefSet {
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
    fn cats_of(&self, first: usize, a: usize, out: &mut Vec<(u8, u8)>) {
        let Some(at) = self.atoms().get(a) else {
            return;
        };
        // (the regimes by index, computed once per text: no compare of
        // two catcode tables)
        if self.reg.get(first).is_some_and(|&f| f != self.reg[a]) {
            return;
        }
        let r = &self.p.regimes[self.reg[a]].1;
        let t = self.text.as_bytes();
        let bytes: &[u8] = match at.kind {
            AtomKind::Cs => {
                let name = at.name(self.text);
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
                && !out.contains(&(b, c))
            {
                out.push((b, c));
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
        while self.claimed[k] {
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
        let excerpt = &self.text[at.start..self.text.len().min(at.start + 40)];
        if self.claimed[a] {
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
        if at.kind != AtomKind::Cs || (self.inner[k] && !self.after_global(k)) {
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
        st.is_none_or(|st| st.iter().any(|x| x == head))
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
        let excerpt = &self.text[at.start..self.text.len().min(at.start + 40)];
        if at.kind == AtomKind::Cs && DEMOTED.contains(&at.name(self.text)) {
            return demoted(at.name(self.text), excerpt);
        }
        if at.kind != AtomKind::Cs
            && let Some(p) = self.prev(a)
            && self.atoms()[p].kind == AtomKind::Cs
            // (a command that is itself an operand, `\\footnote\\dag{…}`,
            // reads nothing: its reader does)
            && !self.claimed[p]
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
        if self.in_align[a] {
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
                format!("{f}: an active character the text does not make"),
                excerpt,
            );
        }
        // (a font selected inside a brace group, not `\\global`ly, is
        // the group's: no `font` definition reaches the engine)
        if self.inner.get(a).copied().unwrap_or(false)
            && !self.after_global(a)
            && let Some(st) = &mut b.state
        {
            st.retain(|h| h != "font");
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
            || (c.is_ascii()
                && (self.defs.active_chars() | self.prelude.active_chars()) >> (c as u32) & 1 == 1)
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
    fn calls_state_split(&self, calls: &[String], outer: &[String], st: &mut Option<Vec<String>>) {
        let (out, inner): (Vec<String>, Vec<String>) =
            calls.iter().cloned().partition(|c| outer.contains(c));
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
                    .map(String::as_str)
                    .filter(|h| !group_local_head(h))
                    .collect();
                add_heads(st, Some(&keep));
            }
        }
    }

    fn calls_state(&self, calls: &[String], st: &mut Option<Vec<String>>) {
        for c in calls {
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
            if row.is_none() && !is_prim(c.as_str()) && !table_param(c) {
                // (a name neither a primitive nor defined: a `\\let`'s
                // value that may be undefined, `\\let\\next\\undefined`;
                // a call of it is unbounded in the may-define set, not
                // in the state)
                if crate::exec::env_flag!("PHITEX_SLICES_TRACE") {
                    eprintln!("slices: state skips undefined \\{c}");
                }
                continue;
            }
            if row.is_none() && crate::exec::env_flag!("PHITEX_SLICES_TRACE") {
                eprintln!("slices: state every head from \\{c}");
            }
            if row.is_some_and(|r| r.contains(&"param:"))
                && crate::exec::env_flag!("PHITEX_SLICES_TRACE")
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
    fn defined_state(&self, name: &str, may_start_par: bool, st: &mut Option<Vec<String>>) {
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
                add_heads(st, Some(&[fam, "aftertoken"]));
            }
            Some(pass::Shape::Quantity(_)) => {
                add_heads(st, Some(&["box:", "lvlfix", "badness", "inspen"]));
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
                if st.is_some() && crate::exec::env_flag!("PHITEX_SLICES_TRACE") {
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
    fn add_set(&self, set: &DefSet, b: &mut Bound, cause: &mut String) -> bool {
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
    fn add_set_parts(&self, set: &DefSet, b: &mut Bound) {
        for r in &set.relied {
            if let Some(f) = Tok::cs(&caret_name(r)).field_name()
                && !b.means.iter().any(|m| m.0 == f)
            {
                b.means.push((f, 1));
            }
        }
        for f in set.names.keys() {
            if let Some(f) = engine_field(f) {
                if !b.names.contains(&f) {
                    b.names.push(f);
                }
            } else if let Some(p) = self.param_field(f) {
                if let Some(p) = p {
                    add_head(&mut b.state, &p);
                }
            } else if let Some(h) = head_of_pass_field(f) {
                if crate::exec::env_flag!("PHITEX_SLICES_TRACE") {
                    eprintln!("slices: head {h} from name {f}");
                }
                add_head(&mut b.state, h);
            }
        }
        for f in set.prefixes.keys() {
            if let Some(p) = engine_prefix(f) {
                if !b.prefixes.contains(&p) {
                    b.prefixes.push(p);
                }
            } else if let Some(p) = self.param_field(f) {
                if let Some(p) = p {
                    add_head(&mut b.state, &p);
                }
            } else if let Some(h) = head_of_pass_field(f) {
                if crate::exec::env_flag!("PHITEX_SLICES_TRACE") {
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
            *cause = format!("\\{name}: a prefix whose command follows elsewhere");
            return false;
        }
        if LOOKTHROUGH.contains(&name) {
            *cause = format!("\\{name}: the command run here is computed");
            return false;
        }
        let Some(&i) = self.item_of.get(&a) else {
            // (an operand of an earlier command: claimed, never run)
            *cause = format!("\\{name}: operand of an earlier command");
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
            if let Some(f) = Tok::cs(&caret_name(r)).field_name()
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
            pass::MayDefine::Names(ds) => {
                add_defs_with(self, ds, false, b);
                true
            }
            pass::MayDefine::Prefixes(ds) => {
                add_defs_with(self, ds, true, b);
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
                    || format!("\\{name}: unbounded"),
                    |c| format!("\\{name}: {c}"),
                );
                if self.prelude.is_defined(name) {
                    // (why the format's body did not bound it)
                    let set = self.defset(name);
                    *cause = format!(
                        "{cause} (format closure: {})",
                        set.unbounded.first().map_or("bounded", String::as_str)
                    );
                }
                if *cause == format!("\\{name}: unbounded") {
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
fn align_atoms(text: &str, atoms: &[Atom]) -> Vec<bool> {
    let mut out = vec![false; atoms.len()];
    let mut depth = 0usize;
    let mut open: Vec<usize> = Vec::new();
    let mut pending = false;
    for (k, a) in atoms.iter().enumerate() {
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
        out[k] = !open.is_empty();
    }
    out
}

fn add_defs_with(an: &Analysis<'_>, ds: &[Def], prefixes: bool, b: &mut Bound) {
    for d in ds {
        if let Some(p) = an.param_field(&d.field) {
            if let Some(p) = p {
                add_head(&mut b.state, &p);
            }
        } else if let Some(h) = head_of_pass_field(&d.field) {
            if crate::exec::env_flag!("PHITEX_SLICES_TRACE") {
                eprintln!("slices: head {h} from def {}", d.field);
            }
            add_head(&mut b.state, h);
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

// (one pass over the atoms, then one over the slices; splitting it
// would only pass its tables around)
#[allow(clippy::too_many_lines)]
fn analyse(text: &str, kind: FileKind, defs: &Defs) -> TextHints {
    let an = Analysis::new(text, kind, defs.clone());
    let (slices, pars) = slices_of(text, &an.p, kind);
    let mut starts = Vec::with_capacity(an.p.atoms.len());
    let mut hints: Vec<Arc<Hint>> = Vec::with_capacity(an.p.atoms.len());
    let mut memo: FxHashMap<String, Arc<str>> = FxHashMap::default();
    let mut intern = |h: &Hint| -> Arc<str> {
        let s = h.encode();
        memo.entry(s.clone())
            .or_insert_with(|| Arc::from(s))
            .clone()
    };
    let mut skip = Vec::with_capacity(an.p.atoms.len());
    // (the atoms a primitive reads: never run on their own; a command
    // a macro is only guessed to read runs in the slice, so it counts)
    let mut prim_operand = vec![false; an.p.atoms.len()];
    for it in &an.p.items {
        if is_prim(&it.name) {
            for c in prim_operand
                .iter_mut()
                .take(it.end.min(an.p.atoms.len()))
                .skip(it.atom + 1)
            {
                *c = true;
            }
        }
    }
    let mut prev_char = false;
    #[allow(clippy::needless_range_loop)]
    for a in 0..an.p.atoms.len() {
        let at = &an.p.atoms[a];
        starts.push(at.start);
        let h = if an.claimed[a] && at.kind == AtomKind::Cs && !prim_operand[a] {
            an.hint_run(a)
        } else {
            an.hint(a)
        };
        // (a guessed operand that is unbounded on its own is a value
        // read, not run: `\\headline={\\tenrm…}`)
        let unbounded_operand = an.claimed[a] && h.bound.is_none();
        hints.push(Arc::new(h));
        let ch = matches!(at.kind, AtomKind::Char(_) | AtomKind::Verb);
        // (a command a macro is only guessed to read still runs in the
        // slice: its rows count in the union)
        skip.push(
            (ch && prev_char)
                || (an.claimed[a]
                    && (at.kind != AtomKind::Cs || prim_operand[a] || unbounded_operand))
                || at.kind == AtomKind::Space,
        );
        prev_char = ch;
    }
    let mut t = TextHints {
        slices,
        starts,
        hints,
        skip,
        pars,
        slice_hints: Vec::new(),
    };
    // each slice's hint: the union of its commands' bounds (a run
    // executes the whole slice); a paragraph's region also defines
    // `\prevgraf` of the list it is in
    let mut slice_hints = Vec::with_capacity(t.slices.len());
    let mut pending: Vec<&'static str> = Vec::new();
    for (i, &(s, e)) in t.slices.iter().enumerate() {
        let mut h = t.at_until(s, e);
        // (whether the run reads on into the next slice: a scan for a
        // keyword or unit there, below; its claim is then the run's)
        let mut reads_on = false;
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
                    // (blanks between: the scan reads past them)
                    if text[ns..ne].trim().is_empty() {
                        reads_on = true;
                        j += 1;
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
        let excerpt = short(&text[s..text.len().min(s + 40)]);
        // (a font selection, `\\bf`, that only an argument or a group
        // inside the slice makes is local to that group: the slice
        // declares it only when a command outside every brace group of
        // the slice selects one, §1217)
        for head in GROUP_LOCAL {
            let k0 = t.starts.partition_point(|&x| x < s);
            let k1 = t.starts.partition_point(|&x| x < e);
            let outside = (k0..k1).any(|k| an.selects_outside(k, head));
            if !outside {
                for b in [h.bound.as_mut(), h.guess.as_mut()].into_iter().flatten() {
                    if let Some(st) = &mut b.state {
                        st.retain(|x| x != head);
                    }
                }
            }
        }
        if t.pars[i] {
            if let Some(b) = &mut h.bound {
                add_heads(&mut b.state, Some(&["pg:"]));
                "region".clone_into(&mut h.cause);
            } else {
                h.cause = format!("region: {}", h.cause);
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
        // (LaTeX's paragraph hooks assign `\\everypar` at every
        // paragraph, the value it has: no definition. A heading's
        // `\\@afterheading`, a list's `\\item` and an environment's
        // end (`\\@doendpe`) set another, which the next paragraph
        // consumes, §1091: a slice calling one declares it)
        if sets_everypar(&text[s..e])
            && let Some(b) = h.bound.as_mut().or(h.guess.as_mut())
        {
            add_head(&mut b.state, "param:everypar");
        }
        let mut out = pending.clone();
        let mut ends = false;
        let k0 = t.starts.partition_point(|&x| x < s);
        for a in k0..t.starts.len() {
            if t.starts[a] >= e {
                break;
            }
            if an.ends_par_at(a) {
                ends = true;
                out.clear();
            }
            if t.skip[a] {
                continue;
            }
            let ah = &*t.hints[a];
            for &n in NORMAL_PARAGRAPH {
                if may_set(ah, n) && !out.contains(&n) {
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
            b.claim = (!reads_on && an.slice_claims_nothing(k0, e)).then_some(0);
        }
        slice_hints.push(intern(&h));
    }
    if crate::exec::env_flag!("PHITEX_SLICES_TRACE") {
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
        }
    }
    t.slice_hints = slice_hints;
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

/// By the text, its kind and the definitions (one set per prelude, so
/// its address identifies it).
fn texts() -> &'static Cache<(u64, u8, usize)> {
    static C: OnceLock<Cache<(u64, u8, usize)>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(FxHashMap::default()))
}

/// Registers a text's analysis under `name` (a string value): a node
/// made from part of that text later (the rest of a slice another
/// command's read-ahead claimed, `Run::leftover`) names it and its
/// offset, and gets the union hint of its text from the whole analysis
/// instead of one computed on its own without the definitions.
pub(crate) fn register_text(name: ValueId, t: &Arc<TextHints>) {
    let mut bn = by_name().lock().unwrap_or_else(PoisonError::into_inner);
    if !bn.get(&name.raw()).is_some_and(|x| Arc::ptr_eq(x, t)) {
        bn.insert(name.raw(), t.clone());
    }
}

fn by_name() -> &'static Cache<u64> {
    static C: OnceLock<Cache<u64>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(FxHashMap::default()))
}

/// Whether a text is written for INITEX's catcodes: its first `{` comes
/// after its own `\catcode` of `{` (plain.tex, latex.ltx, a document
/// run without a format).
fn looks_initex(text: &str) -> bool {
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

/// The regime a text starts in: the kind its name says, unless the text
/// itself sets up its braces.
pub(crate) fn kind_of(text: &str, named: FileKind) -> FileKind {
    if looks_initex(text) {
        FileKind::Initex
    } else {
        named
    }
}

/// The analysis of a document's or prelude's text under the format's
/// definitions `defs` (cached by the text and the definitions).
pub(crate) fn text_hints(text: &str, kind: FileKind, defs: &Defs) -> Arc<TextHints> {
    let kind = kind_of(text, kind);
    let key = (hash_str(text), kind as u8, Arc::as_ptr(defs) as usize);
    if let Some(t) = texts()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
    {
        return t.clone();
    }
    let t = Arc::new(analyse(text, kind, defs));
    let mut c = texts().lock().unwrap_or_else(PoisonError::into_inner);
    if c.len() > 256 {
        c.clear();
    }
    c.insert(key, t.clone());
    t
}

/// The hint of the document slice beginning at byte `s` of the text `t`
/// analyses (looked up once per text by the caller), as
/// the node's extra argument.
pub(crate) fn hint_arg(v: &Values, t: &TextHints, s: usize) -> ValueId {
    match t.slice_hint(s) {
        Some(h) => v.str(h),
        None => v.str(&Hint::any("slice not found", "").encode()),
    }
}

fn by_id() -> &'static Cache<u64> {
    static C: OnceLock<Cache<u64>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(FxHashMap::default()))
}

/// The regime of a file read by `\input`: its text value `id`, named
/// `name`.
pub(crate) fn file_kind_of(v: &Values, id: ValueId, name: ValueId) -> FileKind {
    kind_of(&str_of(v, id), FileKind::from_path(&str_of(v, name)))
}

/// The analysis of a file read by `\input`: its text value `id`, named
/// `name`.
pub(crate) fn file_hints(v: &Values, id: ValueId, name: ValueId) -> Arc<TextHints> {
    let known = by_id()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&id.raw())
        .cloned();
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
            by_id()
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(id.raw(), t.clone());
            t
        }
    };
    let mut bn = by_name().lock().unwrap_or_else(PoisonError::into_inner);
    if !bn.get(&name.raw()).is_some_and(|x| Arc::ptr_eq(x, &t)) {
        bn.insert(name.raw(), t.clone());
    }
    t
}

/// The hint of a node from its arguments: the extra argument when the
/// slice carries one, else its first text frame.
pub(crate) fn hint_of_args(v: &Values, args: &[ValueId]) -> Arc<Hint> {
    static C: OnceLock<Mutex<FxHashMap<u64, Arc<Hint>>>> = OnceLock::new();
    let cache = C.get_or_init(|| Mutex::new(FxHashMap::default()));
    let key = {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for a in args {
            a.raw().hash(&mut h);
        }
        h.finish()
    };
    if let Some(h) = cache
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
    {
        return h.clone();
    }
    let h = Arc::new(compute_hint(v, args));
    let mut c = cache.lock().unwrap_or_else(PoisonError::into_inner);
    if c.len() > 1 << 20 {
        c.clear();
    }
    c.insert(key, h.clone());
    h
}

#[allow(clippy::too_many_lines)]
fn compute_hint(v: &Values, args: &[ValueId]) -> Hint {
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
                let t = file_hints(v, id, name);
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
                let t = by_name()
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(&name.raw())
                    .cloned();
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
                h.cause = format!("{} (text on its own)", h.cause);
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
            if !crate::exec::env_flag!("PHITEX_HINTS_FRAGMENTS") {
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
                        h.cause = format!("{} (expansion tokens)", h.cause);
                    }
                    h
                }
                None => Hint::any("expansion tokens with unusual catcodes", &text),
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

/// Counts since the process began (the fallback when the build's own
/// counters are not read): bounded, any, violations, undeclared.
pub(crate) static COUNTS: [AtomicU64; 7] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

static OFF: AtomicBool = AtomicBool::new(false);

/// Turns the hints off (every node may define anything, as without
/// them) or on again, for comparisons; `PHITEX_HINTS_OFF=1` does the
/// same for a process.
pub fn set_hints_enabled(on: bool) {
    OFF.store(!on, Ordering::Relaxed);
}

/// Whether a slice calls a command that leaves `\\everypar` other
/// than LaTeX's paragraph hooks leave it: a heading (`\\@afterheading`),
/// a list item, an environment's begin or end (`\\@doendpe`), a title.
fn sets_everypar(slice: &str) -> bool {
    const SETS: &[&str] = &[
        "\\section",
        "\\subsection",
        "\\subsubsection",
        "\\paragraph",
        "\\subparagraph",
        "\\chapter",
        "\\part",
        "\\@afterheading",
        "\\@startsection",
        "\\item",
        "\\begin",
        "\\end",
        "\\maketitle",
    ];
    SETS.iter().any(|c| slice.contains(c))
}

/// Whether the node's text says its command opens and closes no scope.
pub(crate) fn effect_none(v: &Values, args: &[ValueId]) -> bool {
    if OFF.load(Ordering::Relaxed) || crate::exec::env_flag!("PHITEX_HINTS_OFF") {
        return false;
    }
    hint_of_args(v, args)
        .bound
        .as_ref()
        .is_some_and(|b| b.no_scope)
}

/// The fields the hint rests on, which the node reads first: the
/// meanings of the names its text calls (`cs:name`, `act:code`) and the
/// catcodes its bytes were read with (`cat:c`).
pub(crate) fn reads(v: &Values, args: &[ValueId]) -> Option<Vec<Field>> {
    if OFF.load(Ordering::Relaxed) || crate::exec::env_flag!("PHITEX_HINTS_OFF") {
        return None;
    }
    let h = hint_of_args(v, args);
    let b = h.bound.as_ref()?;
    let mut out: Vec<Field> = b.means.iter().map(|(n, _)| v.field(n)).collect();
    out.extend(b.cats.iter().map(|(c, _)| v.field(&format!("cat:{c}"))));
    (!out.is_empty()).then_some(out)
}

/// The engine's question, answered from the text.
pub(crate) fn may_define(v: &Values, args: &[ValueId]) -> MayDefine {
    if OFF.load(Ordering::Relaxed) || crate::exec::env_flag!("PHITEX_HINTS_OFF") {
        return MayDefine::Any;
    }
    let h = hint_of_args(v, args);
    if crate::exec::env_flag!("PHITEX_HINTS") {
        let answer = match &h.bound {
            None => "Any".to_owned(),
            Some(b) => format!(
                "Prefixes({} + {:?} + {:?})",
                b.state
                    .as_ref()
                    .map_or_else(|| "every head".to_owned(), |st| st.join(" ")),
                b.names,
                b.prefixes
            ),
        };
        eprintln!("hint {:?}: {answer} ({})", h.excerpt, h.cause);
    }
    h.may_define()
}

/// The accumulators the nodes count into.
pub(crate) const COUNTERS: [&str; 7] = [
    "hints:bounded",
    "hints:any",
    "hints:violations",
    "hints:undeclared",
    "hints:demoted",
    // (nodes whose text is a token list, and those reading source text)
    "frames:tokens",
    "frames:text",
];

/// Classes whose state row still let a run define outside it in the
/// suite: they answer `Any` until each is fixed (with its section) and
/// promoted again.
const DEMOTED: &[&str] = &[
    // (none: each class above was completed from tex.web and promoted)
];

fn demoted(class: &str, excerpt: &str) -> Hint {
    Hint::any(format!("demoted: {class}"), excerpt)
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
    for &(byte, cat) in &b.cats {
        let got = r.cat(u32::from(byte))?;
        if got != cat {
            return Ok(Some(format!(
                "catcode of {:?} is {got}, the hint read it as {cat} ({:?})",
                byte as char, h.excerpt
            )));
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
            return Ok(Some(format!(
                "{name} is {} where the hint assumed {} ({:?})",
                mean_kind(&m),
                if *class == 0 {
                    "a primitive"
                } else {
                    "a definition"
                },
                h.excerpt
            )));
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

/// The definitions a run made outside its bound: the violation, if any.
pub(crate) fn check_defs(r: &mut Run<'_, '_>, h: &Hint) -> Option<String> {
    let b = h.bound.as_ref()?;
    for f in r.def_fields() {
        let name = match &*r.v.get(f.0) {
            Value::Str(s) => s.to_string(),
            _ => continue,
        };
        if !is_cs_field(&name) {
            match &b.state {
                Some(st) if !st.iter().any(|p| name.starts_with(p.as_str())) => {
                    return Some(format!(
                        "undeclared definition of {name} by {:?} (state {st:?}, {})",
                        h.excerpt, h.cause
                    ));
                }
                _ => continue,
            }
        }
        if b.names.contains(&name) || b.prefixes.iter().any(|p| name.starts_with(p.as_str())) {
            continue;
        }
        return Some(format!(
            "undeclared definition of {name} by {:?} (bound {:?} {:?}, {})",
            h.excerpt, b.names, b.prefixes, h.cause
        ));
    }
    None
}

/// `PHITEX_HINTS=2`: the node's definitions, for learning the state
/// each command sets.
pub(crate) fn debug_defs(r: &mut Run<'_, '_>, h: &Hint) {
    if std::env::var("PHITEX_HINTS").is_ok_and(|x| x == "2") {
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
    COUNTS[k].fetch_add(1, Ordering::Relaxed);
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
        let (sl, pars) = slices_of(t, &an.p, FileKind::Document);
        eprintln!("{sl:?} {pars:?}");
        let h = t.find("world").expect("world");
        let k = sl
            .iter()
            .position(|x| x.0 <= h && h < x.1)
            .expect("a slice");
        assert!(pars[k], "{sl:?} {pars:?}");
    }

    #[test]
    fn reset_pending_across_slices() {
        // (`\\parshape` set in one slice, the paragraph ended in the
        // next: that one's `normal_paragraph` resets it, §1070; the one
        // after finds it reset already)
        let t = "\\parshape 1 0pt 100pt text\n\\input x\nmore text\\par\nthird\\par\n";
        let text = analyse(t, FileKind::Document, no_defs());
        let state = |needle: &str| -> Vec<String> {
            let k = text
                .slices
                .iter()
                .position(|&(s, e)| t[s..e].contains(needle))
                .expect("a slice");
            eprintln!("{needle}: {:?}", &t[text.slices[k].0..text.slices[k].1]);
            Hint::decode(&text.slice_hints[k])
                .bound
                .and_then(|b| b.state)
                .expect("a state row")
        };
        assert!(state("more").iter().any(|h| h == "parshape"));
        assert!(!state("third").iter().any(|h| h == "parshape"));
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
        assert!(par.iter().any(|h| h == "pg:") && !par.iter().any(|h| h == "parshape"));
        // the run after it: `\\prevgraf` if a command after it in its
        // text ends the paragraph, and `\\afterassignment`'s row (the
        // region runs on to its end); no dead head (`par:`, `if:`,
        // `lopen:`, `page:`) is ever declared
        assert_eq!(
            st(10).as_deref(),
            Some(&["pg:".to_owned(), "aftertoken".to_owned()][..])
        );
        for off in [0, 5, 10] {
            let v = st(off).expect("some");
            assert!(
                !v.iter()
                    .any(|h| ["par:", "if:", "lopen:", "page:"].contains(&h.as_str())),
                "{off}: {v:?}"
            );
        }
        let d = text.at(t.find("\\def").unwrap()).bound.expect("bounded");
        assert!(d.state.expect("some").contains(&"aftertoken".to_owned()));
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
