//! What a node may define, read from its text by the syntactic pass
//! (`phitex-source`): the engine then makes readers of other fields wait
//! on nothing of it.
//!
//! A node's text is one command (its first atom: a control sequence, a
//! run of characters, a brace, a blank line). The pass bounds the
//! control sequences a command may define (`cs:` and `act:` fields: its
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
//! hint goes into its node's arguments ([`hint_arg`]); a file read by
//! `\input` is analysed once by its text ([`file_hints`]) when its first
//! node asks, and the rest of its lines (nodes whose text frame names
//! the file and an offset) find it by name. Expansion tokens carry no
//! text position and answer `Any`.
//!
//! The pass reads bodies from the text itself and from the format's
//! definitions ([`Defs`]: the prelude's text and every file it read,
//! harvested once per prelude by `api.rs`): a call of a format macro is
//! bounded by its body like a call of the document's own, and a macro
//! whose body ends a paragraph marks a paragraph region's end wherever
//! it is called (OUTPUT.md §5). Without them every region boundary a
//! macro hides was missed, and every paragraph of a document written
//! as macro calls shared one cell (`par:0:…`) in one serial chain.
//!
//! `PHITEX_HINTS=1` prints each distinct node text's answer and cause;
//! `PHITEX_HINTS_OFF=1` answers `Any` for every node (the comparison).

use std::collections::HashMap;
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
}

/// The state fields (heads: a family's prefix with its colon, or a
/// cell's name) a command of this name may set besides control
/// sequences, from tex.web; `None`: a command whose state is not
/// listed here (every head). The alignment state (`align:`), the
/// paragraph (`par:`) and `aftertoken` come from the context, not from
/// the command ([`Analysis::cs_hint`]).
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
        // a paragraph's end (§1094-§1096): `line_break` sets its lines'
        // cells (`par:`), `\\prevgraf` (`pg:`, §1096), the vertical
        // list's opener (`lopen:`); the lines' inserts and `\\vadjust`
        // material (`inspen`, `box:`, `param:`, §1199); the page builder
        // after it (`page:`, `deadcycles`, §1094 build_page); and
        // `normal_paragraph` (§1070)
        "par" => PAR_END,
        // a box's dimension (§1247): the box and its level fix; the
        // dimension's value is scanned with expansion (`if:`, §498)
        "wd" | "ht" | "dp" => &["box:", "lvlfix", "if:"],
        // horizontal material: may start a paragraph ([`starts_par`] adds
        // `par:`), the box level fix (§1090, §1091)
        // (`\\accent` does the assignments before its character: a font,
        // registers, §1123)
        "accent" => &[
            "box:", "lvlfix", "font", "count:", "dimen:", "skip:", "muskip:", "toks:", "param:",
        ],
        "char" | "discretionary" | "-" | "hskip" | "hfil" | "hfill" | "hss" | "hfilneg"
        | "vrule" | "indent" | "noindent" | "unhbox" | "unhcopy" | "box" | "copy" | "leaders"
        | "cleaders" | "xleaders" => &["box:", "lvlfix"],
        // vertical material and removals (§1056, §1105, §1080)
        "vskip" | "vfil" | "vfill" | "vss" | "vfilneg" | "kern" | "hrule" | "unvbox"
        | "unvcopy" | "unskip" | "unpenalty" | "unkern" | "lastbox" => &["box:"],
        // math only (§1136-§1194)
        "mathord" | "mathop" | "mathbin" | "mathrel" | "mathopen" | "mathclose" | "mathpunct"
        | "mathinner" | "underline" | "overline" | "mathaccent" | "radical" | "displaystyle"
        | "textstyle" | "scriptstyle" | "scriptscriptstyle" | "left" | "right" | "middle"
        | "mathchar" | "delimiter" | "displaylimits" | "limits" | "nolimits" | "nonscript"
        | "mathchoice" | "eqno" | "leqno" | "over" | "atop" | "above" | "overwithdelims"
        | "atopwithdelims" | "abovewithdelims" => &["inc"],
        // opening a group: the scope's opener record (`lopen:`, run.rs
        // `finish`; §1063)
        "begingroup" | "bgroup" => &["lopen:"],
        // closing a group: the box's badness, its level fix, inserts (§1100, §1079)
        // and the box a `\\setbox` or box context gets (§1077 box_end)
        // (an output routine's `}` resumes the page builder, §1100)
        "endgroup" | "egroup" => &["badness", "lvlfix", "inspen", "box:", "deadcycles"],
        // arguments expanded while read: a conditional may end (§366, §498)
        "edef" | "xdef" | "csname" | "message" | "errmessage" | "special" => &["if:", "cs:"],
        "mark" => &["if:", "param:"],
        // (e-TeX's mark classes, §1494 ff.)
        "marks" => &["if:", "param:", "markclasses"],
        "write" | "openout" | "closeout" => &["if:", "wopen:", "write:"],
        // (`\\immediate` of a pdfTeX object too, pdfTeX §1528)
        "immediate" => &["if:", "wopen:", "write:", "pdf:"],
        "shipout" | "insert" | "vadjust" | "output" => &["param:"],
        "afterassignment" => &["aftertoken"],
        "aftergroup" => &["ag"],
        // boxes (§1083 begin_box, §1077 \\setbox); a box whose braces close in
        // the same node is packaged there and sets \\\\badness, §1086, §649)
        "hbox" | "vbox" | "vtop" | "vcenter" | "setbox" | "vsplit" => {
            &["box:", "align", "param:", "lvlfix", "badness", "markclasses"]
        }
        // conditionals (§498-§510)
        "if" | "ifx" | "ifnum" | "ifdim" | "ifodd" | "ifvmode" | "ifhmode" | "ifmmode"
        | "ifinner" | "ifvoid" | "ifhbox" | "ifvbox" | "ifcat" | "ifcase" | "iftrue"
        | "iffalse" | "ifdefined" | "ifcsname" | "iffontchar" | "unless" | "else" | "fi" | "or"
        | "ifpdfprimitive" | "ifpdfabsnum" | "ifpdfabsdim" | "ifincsname" | "ifeof" => &["if:"],
        // fonts (§1257 new_font, §1234)
        "font" => &[
            "fd:",
            "fdn:",
            "fontcount",
            "fontid:",
            "fontinfo:",
            "hc:",
            "sc:",
            "if:",
        ],
        "textfont" | "scriptfont" | "scriptscriptfont" => &["fam:"],
        // a font's parameters (§1253)
        "fontdimen" => &["fd:", "fdn:"],
        "hyphenchar" => &["hc:"],
        "skewchar" => &["sc:"],
        // codes (§1232)
        // (the code's value is scanned with expansion: a conditional
        // may end, §1232, §498)
        "catcode" => &["cat:", "if:"],
        "lccode" => &["lc:"],
        "uccode" => &["uc:"],
        "sfcode" => &["sf:"],
        "mathcode" => &["mc:"],
        "delcode" => &["dc:"],
        // registers (§1224-§1238): the register families, the
        // parameters (`\\advance\\tolerance`), a conditional expanded
        // in the value (§498), `\\afterassignment`'s token (§1269)
        "count" | "dimen" | "skip" | "muskip" | "toks" | "advance" | "multiply" | "divide" => &[
            "count:",
            "dimen:",
            "skip:",
            "muskip:",
            "toks:",
            "param:",
            "parshape",
            "if:",
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
        | "pagefilllstretch" | "pageshrink" | "pagedepth" => &["page:"],
        "batchmode" | "nonstopmode" | "scrollmode" | "errorstopmode" | "interactionmode" => {
            &["interaction"]
        }
        // (§960, §934: the pattern trie and exceptions; the list opener
        // the engine records for the braced list, `lopen:`; a
        // conditional in the list's expansion, §961)
        "hyphenation" | "patterns" => &["patterns", "lopen:", "if:"],
        // expansion only (§367, §369)
        "noexpand" => &[],
        // boxes in a box context (§1073 begin_box via §1084)
        "moveleft" | "moveright" | "raise" | "lower" => {
            &["box:", "align", "param:", "lvlfix", "badness"]
        }
        // a language whatsit (§1377) in horizontal material, which may
        // start a paragraph (`par:`, `lopen:`, `pg:`, §1091)
        "setlanguage" => &["box:", "lvlfix", "par:", "lopen:", "pg:"],
        // (they may read or expand anything: every head, §366 \\expandafter,
        // §362 \\input/\\endinput, §1285 \\lowercase/\\uppercase)
        "expandafter" | "input" | "endinput" | "lowercase" | "uppercase" => return None,
        // alignments (§774-§800)
        "halign" | "valign" | "cr" | "crcr" | "noalign" | "omit" | "span" => &["align", "badness"],
        "read" | "readline" | "openin" | "closein" => &["read:"],
        // pdfTeX's objects, streams, random numbers, colour stacks
        _ if n.starts_with("pdf") && crate::prims::PRIMS.iter().any(|p| p.0 == n) => {
            &["pdf:", "if:"]
        }
        _ => {
            if crate::prims::PRIMS.iter().any(|p| p.0 == n) || table_param(n) {
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
const PAR_END: &[&str] = &[
    "par:",
    "pg:",
    "lopen:",
    "inspen",
    "box:",
    "lvlfix",
    "param:",
    "parshape",
    "page:",
    "deadcycles",
    "if:",
];

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
    "lopen:",
    "inc",
    "pg:",
    "param:fam",
    "param:predisplaysize",
    "param:displaywidth",
    "param:displayindent",
];

/// A parameter's own field (`param:<name>`), for a parameter assigned
/// by name (§1228).
fn param_row(name: &str) -> Option<&'static [&'static str]> {
    static ROWS: OnceLock<HashMap<&'static str, [&'static str; 1]>> = OnceLock::new();
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
            }
            Some(b) => {
                o.push('B');
                o.push('\x1e');
                o.push_str(&self.cause);
                o.push('\x1e');
                o.push_str(&self.excerpt);
                o.push('\x1e');
                o.push_str(&b.names.join("\x1f"));
                o.push('\x1e');
                o.push_str(&b.prefixes.join("\x1f"));
                o.push('\x1e');
                let cats: Vec<String> = b.cats.iter().map(|(c, k)| format!("{c}\x1d{k}")).collect();
                o.push_str(&cats.join("\x1f"));
                o.push('\x1e');
                let means: Vec<String> =
                    b.means.iter().map(|(n, k)| format!("{n}\x1d{k}")).collect();
                o.push_str(&means.join("\x1f"));
                o.push('\x1e');
                match &b.state {
                    None => o.push('*'),
                    Some(st) => o.push_str(&st.join("\x1f")),
                }
                o.push('\x1e');
                o.push(if b.no_scope { 'N' } else { 'P' });
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
            return Hint {
                bound: None,
                cause: if cause.is_empty() {
                    "hint not decodable".to_owned()
                } else {
                    cause
                },
                excerpt,
            };
        }
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
        Hint {
            bound: Some(Bound {
                names,
                prefixes,
                cats,
                means,
                state,
                no_scope,
            }),
            cause,
            excerpt,
        }
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
    hints: Vec<Arc<str>>,
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
        self.at_until(off, usize::MAX)
    }

    /// The hint of the text from byte `off` to byte `end`: the union of
    /// the bounds of the commands in it (a run executes all of them: a
    /// line of a file read by `\input`, a region).
    fn at_until(&self, off: usize, end: usize) -> Hint {
        let k = self.starts.partition_point(|&s| s < off);
        let mut acc: Option<Bound> = None;
        let mut first: Option<Hint> = None;
        let mut a = k;
        while let Some(&s) = self.starts.get(a)
            && s < end
        {
            a += 1;
            if self.skip[a - 1] {
                continue;
            }
            let h = Hint::decode(&self.hints[a - 1]);
            if first.is_none() {
                first = Some(h.clone());
            }
            match (&h.bound, &mut acc) {
                (None, _) => return h,
                (Some(b), Some(acc)) => union(acc, b),
                (Some(b), None) => acc = Some(b.clone()),
            }
        }
        match (first, acc) {
            (Some(mut h), Some(b)) => {
                h.bound = Some(b);
                h
            }
            (Some(h), None) => h,
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
        return if text.is_empty() { (Vec::new(), Vec::new()) } else { (vec![(0, text.len())], vec![true]) };
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
                p.items
                    .get(i + 1)
                    .map_or(it.end, |x| x.atom)
                    .max(it.end)
            }
            _ => it.end,
        };
        for c in claimed.iter_mut().take(end.min(n)).skip(it.atom + 1) {
            *c = true;
        }
    }
    // the operands of a primitive (`\sfcode`\)=0`): never a slice's
    // start, so a command and what it reads are one node (a macro's
    // operands are the pass's guess and do not count)
    let mut operand = vec![false; n];
    for it in &p.items {
        if crate::prims::PRIMS.iter().any(|q| q.0 == it.name) {
            for c in operand.iter_mut().take(it.end.min(n)).skip(it.atom + 1) {
                *c = true;
            }
        }
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
    // inside a brace group or a `\begingroup` pair opened at depth 0:
    // the slice goes on to its end (a group is never split between nodes)
    let mut inside = vec![false; n];
    for (k, a) in p.atoms.iter().enumerate() {
        // (a brace knows its mate's atom index; `p.braces` indexes tokens)
        if a.kind == AtomKind::Open && a.mate != usize::MAX && a.mate > k && depth[k] == 0 {
            for x in inside.iter_mut().take(a.mate + 1).skip(k + 1) {
                *x = true;
            }
        }
    }
    for g in &p.groups {
        if let (Some(o), Some(c)) = (g.open, g.close)
            && depth.get(o) == Some(&0)
        {
            for x in inside.iter_mut().take(c + 1).skip(o + 1) {
                *x = true;
            }
        }
    }
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
                AtomKind::Cs => starts_par(a.name(text)),
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
    let mut starts: Vec<usize> = vec![0];
    let mut prev_char = false;
    for (k, a) in p.atoms.iter().enumerate() {
        let ch = matches!(a.kind, AtomKind::Char(_) | AtomKind::Verb);
        let inside_if = ifd[k] > 0 && (a.kind != AtomKind::Cs || !CONDITIONALS.contains(&a.name(text)));
        // (an operand a primitive reads never starts a slice; a macro's
        // operand range is the pass's guess and does not cut)
        if !(ch && prev_char) && !inside_if && !inside[k] && !operand[k] {
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
}

/// The analysis of one text under `kind`'s regime.
struct Analysis<'t> {
    text: &'t str,
    p: Pass,
    defs: Definitions,
    /// Item index by atom.
    item_of: HashMap<usize, usize>,
    /// Atoms an earlier item claims (operands: never run on their own).
    claimed: Vec<bool>,
    /// Atoms textually inside an alignment's braces (`\\halign{`…`}`).
    in_align: Vec<bool>,
    memo: std::cell::RefCell<HashMap<String, DefSet>>,
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
        Analysis {
            text,
            p,
            defs,
            item_of,
            claimed,
            in_align,
            memo: std::cell::RefCell::new(HashMap::new()),
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
        let r = self.p.regime_at(at.tok);
        if self
            .atoms()
            .get(first)
            .is_some_and(|f| self.p.regime_at(f.tok) != r)
        {
            return;
        }
        let bytes: Vec<u8> = match at.kind {
            AtomKind::Cs => {
                let name = at.name(self.text);
                if name.bytes().all(|b| r.catcode(b) == Some(11)) {
                    self.text.as_bytes()[at.start..at.end].to_vec()
                } else {
                    vec![self.text.as_bytes()[at.start]]
                }
            }
            AtomKind::Char(_) | AtomKind::Open | AtomKind::Close | AtomKind::Param(_) => {
                self.text.as_bytes()[at.start..at.end].to_vec()
            }
            AtomKind::Space => vec![b' '],
            AtomKind::Par | AtomKind::Verb => Vec::new(),
        };
        for b in bytes {
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
        if at.kind == AtomKind::Cs && DEMOTED.contains(&at.name(self.text)) {
            return demoted(at.name(self.text), excerpt);
        }
        if at.kind != AtomKind::Cs
            && let Some(p) = self.prev(a)
            && self.atoms()[p].kind == AtomKind::Cs
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
                // (a run appends to its paragraph's cells, `par:<anchor>:…`,
                // whether or not it starts the paragraph; the engine's
                // narrowing keeps the anchor its own)
                add_head(&mut b.state, "par:");
                if self.may_start_par(a) {
                    // (a paragraph begun: its list's opener and
                    // `\\prevgraf` set to 0, §1091)
                    add_head(&mut b.state, "lopen:");
                }
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
                add_heads(&mut b.state, Some(NORMAL_PARAGRAPH));
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
        b.no_scope = ok && self.no_scope(a);
        if ok {
            opener(&mut b.state, at.kind == AtomKind::Open);
            if cause.is_empty() {
                "bounded".clone_into(&mut cause);
            }
            Hint {
                bound: Some(b),
                cause,
                excerpt: short(excerpt),
            }
        } else {
            Hint::any(cause, excerpt)
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
    fn format_only(&self, name: &str) -> bool {
        // (`PHITEX_TRUST_FORMAT=1`: the closure over a format body is a
        // bound like the text's own, for the engine that refutes a
        // declaration instead of failing on it)
        if crate::exec::env_flag!("PHITEX_TRUST_FORMAT") {
            return false;
        }
        !self.defs.is_defined(name) && self.prelude.is_defined(name)
    }

    /// A macro called by name (`\par` for a blank line, an active
    /// character).
    fn call_hint(&self, name: &str, b: &mut Bound, cause: &mut String) -> bool {
        if self.format_only(name) {
            *cause = format!("\\{name}: a format macro (its body is not a bound)");
            return false;
        }
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
    fn calls_state(&self, calls: &[String], st: &mut Option<Vec<String>>) {
        for c in calls {
            if st.is_none() {
                return;
            }
            if self.is_defined(c) {
                // (a name the text defines: its body's calls are in the
                // list already)
                add_head(st, "lopen:");
                self.defined_state(c, true, st);
                continue;
            }
            if starts_par(c) {
                add_heads(st, Some(&["par:", "lopen:", "pg:"]));
            }
            add_heads(st, class_state(c));
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
            Some(pass::Shape::Quantity('i' | 'd' | 'g' | 't')) => {
                add_heads(st, class_state("count"));
            }
            Some(pass::Shape::Quantity(_)) => {
                add_heads(st, Some(&["box:", "lvlfix", "badness", "inspen", "align"]));
                if may_start_par {
                    add_heads(st, Some(&["par:", "pg:"]));
                }
            }
            Some(pass::Shape::Alias(t)) => {
                if starts_par(&t) && may_start_par {
                    add_heads(st, Some(&["par:", "pg:"]));
                }
                if !self.is_defined(&t) {
                    add_heads(st, class_state(&t));
                }
            }
            Some(pass::Shape::Cond) => add_head(st, "if:"),
            Some(pass::Shape::Macro(_)) => {}
            // (definitions that disagree, or one the pass cannot read)
            Some(pass::Shape::Opaque(_)) | None => *st = None,
        }
    }

    /// The set's fields and relied macros into the bound.
    fn add_set(&self, set: &DefSet, b: &mut Bound, cause: &mut String) -> bool {
        if let Some(u) = set.unbounded.first() {
            cause.clone_from(u);
            return false;
        }
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
            } else if let Some(h) = head_of_pass_field(f) {
                add_head(&mut b.state, h);
            }
        }
        for f in set.prefixes.keys() {
            if let Some(p) = engine_prefix(f) {
                if !b.prefixes.contains(&p) {
                    b.prefixes.push(p);
                }
            } else if let Some(h) = head_of_pass_field(f) {
                add_head(&mut b.state, h);
            }
        }
        self.calls_state(&set.calls, &mut b.state);
        true
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
        if self.format_only(name) {
            *cause = format!("\\{name}: a format macro (its body is not a bound)");
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
                if o.starts_with("pdf") && crate::prims::PRIMS.iter().any(|p| p.0 == o) {
                    add_head(&mut b.state, "pdf:");
                } else if class_state(o) == Some(&["if:"][..]) {
                    // (a conditional expanded in an operand, §498)
                    add_head(&mut b.state, "if:");
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
            add_head(&mut b.state, "lopen:");
            self.defined_state(name, self.may_start_par(a), &mut b.state);
        } else {
            add_heads(&mut b.state, class_state(name));
        }
        if name == "font" {
            // (§1257: the font identifier's frozen twin)
            b.prefixes.push("cs:\u{10FFFE}F".to_owned());
        }
        if starts_par(name) && self.may_start_par(a) {
            add_heads(&mut b.state, Some(&["par:", "lopen:", "pg:"]));
        }
        if matches!(name, "par" | "tex_par:D") || it.hints.implicit == pass::Implicit::EndsParagraph
        {
            // (a paragraph's end: its lines, `\\prevgraf`, §1096;
            // `normal_paragraph` resets `\\looseness`, `\\hangindent`,
            // `\\hangafter`, `\\parshape`, `\\interlinepenalties`, §1070)
            add_heads(&mut b.state, Some(PAR_END));
            add_heads(&mut b.state, Some(NORMAL_PARAGRAPH));
        }
        if self.after_afterassignment(first) {
            add_head(&mut b.state, "aftertoken");
        }
        self.calls_state(&it.hints.calls, &mut b.state);
        // (a macro's run expands its body inline: the state of every
        // command its body calls, transitively, the closure's calls)
        if !crate::prims::PRIMS.iter().any(|q| q.0 == name) {
            let set = self.defset(name);
            self.calls_state(&set.calls, &mut b.state);
        }
        match &it.hints.may_define {
            pass::MayDefine::Nothing => true,
            pass::MayDefine::Names(ds) => {
                add_defs(ds, false, b);
                true
            }
            pass::MayDefine::Prefixes(ds) => {
                add_defs(ds, true, b);
                true
            }
            pass::MayDefine::Any => {
                *cause = it.hints.cause.map_or_else(
                    || format!("\\{name}: unbounded"),
                    |c| format!("\\{name}: {c}"),
                );
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

/// A command that opens a list or a scope (a group, a paragraph, a box,
/// math, an alignment, an insert) records its opener when it ends
/// (`lopen:<id>`, run.rs `finish`): every class that may open one.
fn opener(st: &mut Option<Vec<String>>, open: bool) {
    let opens = st.as_ref().is_some_and(|v| {
        v.iter()
            .any(|h| matches!(h.as_str(), "box:" | "inc" | "align" | "param:" | "lvlfix"))
    });
    if open || opens {
        add_head(st, "lopen:");
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

fn add_defs(ds: &[Def], prefixes: bool, b: &mut Bound) {
    for d in ds {
        if let Some(h) = head_of_pass_field(&d.field) {
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

fn analyse(text: &str, kind: FileKind, defs: &Defs) -> TextHints {
    let an = Analysis::new(text, kind, defs.clone());
    let (slices, pars) = slices_of(text, &an.p, kind);
    let mut starts = Vec::with_capacity(an.p.atoms.len());
    let mut hints: Vec<Arc<str>> = Vec::with_capacity(an.p.atoms.len());
    let mut memo: HashMap<String, Arc<str>> = HashMap::new();
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
        if crate::prims::PRIMS.iter().any(|q| q.0 == it.name) {
            for c in prim_operand.iter_mut().take(it.end.min(an.p.atoms.len())).skip(it.atom + 1) {
                *c = true;
            }
        }
    }
    let mut prev_char = false;
    for a in 0..an.p.atoms.len() {
        let at = &an.p.atoms[a];
        starts.push(at.start);
        let h = an.hint(a);
        hints.push(intern(&h));
        let ch = matches!(at.kind, AtomKind::Char(_) | AtomKind::Verb);
        // (a command a macro is only guessed to read still runs in the
        // slice: its rows count in the union)
        skip.push(
            (ch && prev_char)
                || (an.claimed[a] && (at.kind != AtomKind::Cs || prim_operand[a]))
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
    for (i, &(s, e)) in t.slices.iter().enumerate() {
        let mut h = t.at_until(s, e);
        let excerpt = short(&text[s..text.len().min(s + 40)]);
        if t.pars[i] {
            match &mut h.bound {
                Some(b) => {
                    add_head(&mut b.state, "pg:");
                    "region".clone_into(&mut h.cause);
                }
                None => h.cause = format!("region: {}", h.cause),
            }
            h.excerpt = excerpt;
        }
        slice_hints.push(intern(&h));
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

type Cache<K> = Mutex<HashMap<K, Arc<TextHints>>>;

/// By the text, its kind and the definitions (one set per prelude, so
/// its address identifies it).
fn texts() -> &'static Cache<(u64, u8, usize)> {
    static C: OnceLock<Cache<(u64, u8, usize)>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
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
    C.get_or_init(|| Mutex::new(HashMap::new()))
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
    C.get_or_init(|| Mutex::new(HashMap::new()))
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
    static C: OnceLock<Mutex<HashMap<u64, Arc<Hint>>>> = OnceLock::new();
    let cache = C.get_or_init(|| Mutex::new(HashMap::new()));
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
            if !toks.is_empty()
                && toks
                    .iter()
                    .all(|t| matches!(t, Tok::C(_)) && matches!(t.cat(), Some(10..=12)))
            {
                let mut b = Bound {
                    state: Some(Vec::new()),
                    no_scope: true,
                    ..Bound::default()
                };
                // (a run that reaches a cell's end in an alignment, §789:
                // the template's v part runs there, and may hold a
                // conditional, `if:`)
                add_heads(
                    &mut b.state,
                    Some(&[
                        "par:", "pg:", "lopen:", "box:", "lvlfix", "align", "badness", "if:",
                    ]),
                );
                return Hint {
                    bound: Some(b),
                    cause: "characters (expansion tokens)".to_owned(),
                    excerpt: String::new(),
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
                    match t {
                        Tok::S(n) => {
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
                            text.push_str(n);
                            if word {
                                text.push(' ');
                            }
                        }
                        Tok::C(_) => {
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
    add_heads(&mut b.state, Some(&["page:", "deadcycles"]));
    add_head(&mut b.state, "par:");
    add_heads(&mut b.state, Some(NORMAL_PARAGRAPH));
    add_head(&mut b.state, "pg:");
    add_head(&mut b.state, "lopen:");
    add_head(&mut b.state, "aftertoken");
    add_head(&mut b.state, "align");
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
pub(crate) fn check_defs(r: &Run<'_, '_>, h: &Hint) -> Option<String> {
    let b = h.bound.as_ref()?;
    for (f, _, _) in &r.defs {
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
pub(crate) fn debug_defs(r: &Run<'_, '_>, h: &Hint) {
    if std::env::var("PHITEX_HINTS").is_ok_and(|x| x == "2") {
        let mut fs: Vec<String> = r
            .defs
            .iter()
            .filter_map(|(f, _, _)| match &*r.v.get(f.0) {
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
        let an = Analysis::new(t, FileKind::Document, no_defs().clone());
        for (k, a) in an.p.atoms.iter().enumerate() {
            eprintln!("{k} {:?} {:?} claimed {}", a.kind, &t[a.start..a.end], an.claimed[k]);
        }
        for it in &an.p.items {
            eprintln!("item {} {}..{} {:?}", it.name, it.atom, it.end, it.hints.implicit);
        }
        let (sl, pars) = slices_of(t, &an.p, FileKind::Document);
        eprintln!("{sl:?} {pars:?}");
        let h = t.find("\\hello\\").expect("call");
        let k = sl.iter().position(|x| x.0 <= h && h < x.1).expect("a slice");
        assert!(pars[k], "{sl:?} {pars:?}");
    }

    #[test]
    fn paragraph_and_conditional_classes() {
        let t = "\\par\nText here \\ifnum1=1 \\fi\n\\afterassignment\\x\\def\\y{}\n";
        let text = analyse(t, FileKind::Document, no_defs());
        let st = |off: usize| text.at(off).bound.expect("bounded").state;
        // (`\\par` is bounded: a paragraph's end, §1094-§1096, §1070)
        let par = st(0).expect("a state row");
        assert!(par.iter().any(|h| h == "pg:") && par.iter().any(|h| h == "parshape"));
        // the run after it is in one: the list, and `\\prevgraf` if a
        // command after it in its text ends the paragraph
        assert_eq!(
            st(10).as_deref(),
            Some(&["par:".to_owned(), "pg:".to_owned()][..])
        );
        // a run of text after \par may start a paragraph
        assert!(st(5).expect("some").contains(&"par:".to_owned()));
        // and the run after it is in one: no list opened
        assert!(st(5).expect("some").contains(&"lopen:".to_owned()));
        assert!(!st(10).expect("some").contains(&"lopen:".to_owned()));
        // (a conditional, when bounded, may change the if stack)
        if let Some(b) = text.at(15).bound {
            assert!(b.state.expect("some").contains(&"if:".to_owned()));
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
        for f in [
            "pg:1",
            "aftertoken",
            "cat:64",
            "count:10",
            "align:open",
            "fontinfo:2",
        ] {
            assert!(ps.iter().any(|p| f.starts_with(p.as_str())), "{f}");
        }
        for f in ["cs:foo", "act:126"] {
            assert!(!ps.iter().any(|p| f.starts_with(p.as_str())), "{f}");
        }
    }
}

