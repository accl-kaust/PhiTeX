//! Values: tokens, meanings, glue and the codes TeX gives them.
//!
//! A token is `Int(cat << 21 | char)` for a character (an active character
//! has category 13) or `Str("\\name")` for a control sequence (§289).
//! A meaning is a `List` tagged by its first element (§222–§225).

use std::rc::Rc;
use std::sync::OnceLock;

use phitex_defuse::{Field, FxHashMap, Value, ValueId, Values};
use phitex_typeset::glue::{Glue, Order};

// Command codes (§207–§210), the table whole.
#[allow(dead_code)]
pub(crate) mod cmd {
    pub const RELAX: u8 = 0;
    pub const LEFT_BRACE: u8 = 1;
    pub const RIGHT_BRACE: u8 = 2;
    pub const MATH_SHIFT: u8 = 3;
    pub const TAB_MARK: u8 = 4;
    pub const CAR_RET: u8 = 5;
    pub const MAC_PARAM: u8 = 6;
    pub const SUP_MARK: u8 = 7;
    pub const SUB_MARK: u8 = 8;
    pub const IGNORE: u8 = 9;
    pub const SPACER: u8 = 10;
    pub const LETTER: u8 = 11;
    pub const OTHER_CHAR: u8 = 12;
    pub const ACTIVE_CHAR: u8 = 13;
    pub const PAR_END: u8 = 13;
    pub const COMMENT: u8 = 14;
    pub const STOP: u8 = 14;
    pub const INVALID_CHAR: u8 = 15;
    pub const DELIM_NUM: u8 = 15;
    pub const CHAR_NUM: u8 = 16;
    pub const MATH_CHAR_NUM: u8 = 17;
    pub const MARK: u8 = 18;
    pub const XRAY: u8 = 19;
    pub const MAKE_BOX: u8 = 20;
    pub const HMOVE: u8 = 21;
    pub const VMOVE: u8 = 22;
    pub const UN_HBOX: u8 = 23;
    pub const UN_VBOX: u8 = 24;
    pub const REMOVE_ITEM: u8 = 25;
    pub const HSKIP: u8 = 26;
    pub const VSKIP: u8 = 27;
    pub const MSKIP: u8 = 28;
    pub const KERN: u8 = 29;
    pub const MKERN: u8 = 30;
    pub const LEADER_SHIP: u8 = 31;
    pub const HALIGN: u8 = 32;
    pub const VALIGN: u8 = 33;
    pub const NO_ALIGN: u8 = 34;
    pub const VRULE: u8 = 35;
    pub const HRULE: u8 = 36;
    pub const INSERT: u8 = 37;
    pub const VADJUST: u8 = 38;
    pub const IGNORE_SPACES: u8 = 39;
    pub const AFTER_ASSIGNMENT: u8 = 40;
    pub const AFTER_GROUP: u8 = 41;
    pub const BREAK_PENALTY: u8 = 42;
    pub const START_PAR: u8 = 43;
    pub const ITAL_CORR: u8 = 44;
    pub const ACCENT: u8 = 45;
    pub const MATH_ACCENT: u8 = 46;
    pub const DISCRETIONARY: u8 = 47;
    pub const EQ_NO: u8 = 48;
    pub const LEFT_RIGHT: u8 = 49;
    pub const MATH_COMP: u8 = 50;
    pub const LIMIT_SWITCH: u8 = 51;
    pub const ABOVE: u8 = 52;
    pub const MATH_STYLE: u8 = 53;
    pub const MATH_CHOICE: u8 = 54;
    pub const NON_SCRIPT: u8 = 55;
    pub const VCENTER: u8 = 56;
    pub const CASE_SHIFT: u8 = 57;
    pub const MESSAGE: u8 = 58;
    pub const EXTENSION: u8 = 59;
    pub const IN_STREAM: u8 = 60;
    pub const BEGIN_GROUP: u8 = 61;
    pub const END_GROUP: u8 = 62;
    pub const OMIT: u8 = 63;
    pub const EX_SPACE: u8 = 64;
    pub const NO_BOUNDARY: u8 = 65;
    pub const RADICAL: u8 = 66;
    pub const END_CS_NAME: u8 = 67;
    pub const CHAR_GIVEN: u8 = 68;
    pub const MATH_GIVEN: u8 = 69;
    pub const LAST_ITEM: u8 = 70;
    pub const MAX_NON_PREFIXED: u8 = 70;
    pub const TOKS_REGISTER: u8 = 71;
    pub const ASSIGN_TOKS: u8 = 72;
    pub const ASSIGN_INT: u8 = 73;
    pub const ASSIGN_DIMEN: u8 = 74;
    pub const ASSIGN_GLUE: u8 = 75;
    pub const ASSIGN_MU_GLUE: u8 = 76;
    pub const ASSIGN_FONT_DIMEN: u8 = 77;
    pub const ASSIGN_FONT_INT: u8 = 78;
    pub const SET_AUX: u8 = 79;
    pub const SET_PREV_GRAF: u8 = 80;
    pub const SET_PAGE_DIMEN: u8 = 81;
    pub const SET_PAGE_INT: u8 = 82;
    pub const SET_BOX_DIMEN: u8 = 83;
    pub const SET_SHAPE: u8 = 84;
    pub const DEF_CODE: u8 = 85;
    pub const DEF_FAMILY: u8 = 86;
    pub const SET_FONT: u8 = 87;
    pub const DEF_FONT: u8 = 88;
    pub const REGISTER: u8 = 89;
    pub const MAX_INTERNAL: u8 = 89;
    pub const ADVANCE: u8 = 90;
    pub const MULTIPLY: u8 = 91;
    pub const DIVIDE: u8 = 92;
    pub const PREFIX: u8 = 93;
    pub const LET: u8 = 94;
    pub const SHORTHAND_DEF: u8 = 95;
    pub const READ_TO_CS: u8 = 96;
    pub const DEF: u8 = 97;
    pub const SET_BOX: u8 = 98;
    pub const HYPH_DATA: u8 = 99;
    pub const SET_INTERACTION: u8 = 100;
    pub const MAX_COMMAND: u8 = 100;
    pub const UNDEFINED_CS: u8 = 101;
    pub const EXPAND_AFTER: u8 = 102;
    pub const NO_EXPAND: u8 = 103;
    pub const INPUT: u8 = 104;
    pub const IF_TEST: u8 = 105;
    pub const FI_OR_ELSE: u8 = 106;
    pub const CS_NAME: u8 = 107;
    pub const CONVERT: u8 = 108;
    pub const THE: u8 = 109;
    pub const TOP_BOT_MARK: u8 = 110;
    pub const CALL: u8 = 111;
    pub const LONG_CALL: u8 = 112;
    pub const OUTER_CALL: u8 = 113;
    pub const LONG_OUTER_CALL: u8 = 114;
    pub const END_TEMPLATE: u8 = 115;
    pub const DONT_EXPAND: u8 = 116;
}

// eqtb regions (§222–§236), as tex.web numbers them.
pub(crate) const SKIP_BASE: i32 = 2900;
pub(crate) const MU_SKIP_BASE: i32 = 3156;
pub(crate) const OUTPUT_ROUTINE_LOC: i32 = 3413;
pub(crate) const TOKS_BASE: i32 = 3422;
pub(crate) const MATH_FONT_BASE: i32 = 3935;
pub(crate) const CAT_CODE_BASE: i32 = 3983;
pub(crate) const LC_CODE_BASE: i32 = 4239;
pub(crate) const UC_CODE_BASE: i32 = 4495;
pub(crate) const SF_CODE_BASE: i32 = 4751;
pub(crate) const MATH_CODE_BASE: i32 = 5007;
pub(crate) const DEL_CODE_BASE: i32 = 5574;
pub(crate) const COUNT_BASE: i32 = 5318;
pub(crate) const SCALED_BASE: i32 = 5851;
/// `no_expand_flag` (§358).
pub(crate) const NO_EXPAND_FLAG: i32 = 257;

/// Group codes (§269).
#[allow(dead_code)]
pub(crate) mod group {
    pub const BOTTOM: i64 = 0;
    pub const SIMPLE: i64 = 1;
    pub const HBOX: i64 = 2;
    pub const ADJUSTED_HBOX: i64 = 3;
    pub const VBOX: i64 = 4;
    pub const VTOP: i64 = 5;
    pub const ALIGN: i64 = 6;
    pub const NO_ALIGN: i64 = 7;
    pub const OUTPUT: i64 = 8;
    pub const MATH: i64 = 9;
    pub const DISC: i64 = 10;
    pub const INSERT: i64 = 11;
    pub const VCENTER: i64 = 12;
    pub const MATH_CHOICE: i64 = 13;
    pub const SEMI_SIMPLE: i64 = 14;
    pub const MATH_SHIFT: i64 = 15;
    pub const MATH_LEFT: i64 = 16;
}

/// A token (§289): a character with its category, or a control sequence.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Tok {
    /// `cat << 21 | char`; category 13 is an active character.
    C(u32),
    /// A control sequence's name (frozen ones start with `\u{10FFFE}`).
    S(Rc<str>),
}

impl Tok {
    pub(crate) fn ch(cat: u8, c: u32) -> Tok {
        Tok::C(u32::from(cat) << 21 | c)
    }

    pub(crate) fn cs(name: &str) -> Tok {
        Tok::S(Rc::from(name))
    }

    /// The category of a character token.
    pub(crate) fn cat(&self) -> Option<u8> {
        match self {
            Tok::C(x) => Some(u8::try_from(x >> 21).unwrap_or(0)),
            Tok::S(_) => None,
        }
    }

    /// The character code of a character token.
    pub(crate) fn chr(&self) -> u32 {
        match self {
            Tok::C(x) => x & 0x1F_FFFF,
            Tok::S(_) => 0,
        }
    }

    pub(crate) fn is_cs_or_active(&self) -> bool {
        matches!(self, Tok::S(_)) || self.cat() == Some(13)
    }

    /// The field holding this token's meaning, for control sequences and
    /// active characters.
    pub(crate) fn field_name(&self) -> Option<String> {
        match self {
            Tok::S(n) => Some(format!("cs:{n}")),
            Tok::C(_) if self.cat() == Some(13) => Some(format!("act:{}", self.chr())),
            Tok::C(_) => None,
        }
    }

    pub(crate) fn encode(&self, v: &Values) -> ValueId {
        match self {
            Tok::C(x) => v.int(i64::from(*x)),
            Tok::S(n) => v.str(&format!("\\{n}")),
        }
    }

    pub(crate) fn decode(v: &Values, id: ValueId) -> Tok {
        match &*v.get(id) {
            Value::Int(i) => Tok::C(u32::try_from(*i).unwrap_or(0)),
            Value::Str(s) => Tok::S(Rc::from(s.strip_prefix('\\').unwrap_or(s))),
            _ => Tok::C(0),
        }
    }
}

/// The space token `␣₁₀` (§289).
pub(crate) fn space_tok() -> Tok {
    Tok::ch(10, 32)
}

pub(crate) fn toks_value(v: &Values, toks: &[Tok]) -> ValueId {
    v.list(toks.iter().map(|t| t.encode(v)).collect())
}

pub(crate) fn toks_of(v: &Values, id: Option<ValueId>) -> Vec<Tok> {
    match id {
        Some(id) => decoded(
            v,
            id,
            |d| &mut d.toks,
            || match &*v.get(id) {
                Value::List(xs) => xs.iter().map(|&x| Tok::decode(v, x)).collect(),
                _ => Vec::new(),
            },
        ),
        None => Vec::new(),
    }
}

/// Each thread's decoded values, by id: an id is a content hash, so a
/// decoding never goes stale, and a value being decoded is live (a
/// swept id is never asked for again). Saves the value reads of a token
/// list or meaning read again by later runs on the same worker.
#[derive(Default)]
struct Decoded {
    instance: u64,
    toks: FxHashMap<ValueId, Vec<Tok>>,
    means: FxHashMap<ValueId, Mean>,
    glues: FxHashMap<ValueId, Glue>,
}

/// Entries kept per map before it is emptied (a bound, not a policy).
const DECODED_MAX: usize = 1 << 15;

thread_local! {
    static DECODED: std::cell::RefCell<Decoded> = std::cell::RefCell::default();
}

/// `make()`, the decoding of `id`, through this thread's cache.
fn decoded<T: Clone>(
    v: &Values,
    id: ValueId,
    map: fn(&mut Decoded) -> &mut FxHashMap<ValueId, T>,
    make: impl FnOnce() -> T,
) -> T {
    let hit = DECODED.with(|c| {
        let mut c = c.borrow_mut();
        if c.instance != v.instance() {
            *c = Decoded {
                instance: v.instance(),
                ..Decoded::default()
            };
        }
        map(&mut c).get(&id).cloned()
    });
    if let Some(x) = hit {
        return x;
    }
    let x = make();
    DECODED.with(|c| {
        let mut c = c.borrow_mut();
        let m = map(&mut c);
        if m.len() >= DECODED_MAX {
            m.clear();
        }
        m.insert(id, x.clone());
    });
    x
}

pub(crate) fn list_of(v: &Values, id: ValueId) -> Vec<ValueId> {
    match &*v.get(id) {
        Value::List(xs) => xs.to_vec(),
        Value::Seq(_) | Value::Nil => v.seq_to_vec(id),
        _ => Vec::new(),
    }
}

pub(crate) fn int_of(v: &Values, id: ValueId) -> i64 {
    v.as_int(id).unwrap_or(0)
}

pub(crate) fn int_at(v: &Values, id: ValueId, k: usize) -> i64 {
    match &*v.get(id) {
        Value::List(xs) => xs.get(k).map_or(0, |&x| int_of(v, x)),
        _ => 0,
    }
}

pub(crate) fn str_of(v: &Values, id: ValueId) -> String {
    match &*v.get(id) {
        Value::Str(s) => s.to_string(),
        _ => String::new(),
    }
}

pub(crate) fn field_name(v: &Values, f: Field) -> String {
    str_of(v, f.0)
}

/// A meaning (§222): what a control sequence or active character is.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Mean {
    Prim(&'static str, u8, i32),
    Macro(Rc<Macro>),
    Char(u8, u32),
    Undefined,
    Font(i64),
    CharDef(i64),
    MathCharDef(i64),
    /// `\countdef` etc.: the command and the eqtb location.
    Reg(u8, i32),
    /// The marker of `\noexpand` (§358).
    NotExpanded,
}

/// A macro (§210, §473): flags (1 `\long`, 2 `\outer`), its parameter
/// text (match tokens are `Tok::C(14 << 21 | char)`), its body
/// (parameters are `Tok::C(5 << 21 | n)`).
#[derive(Debug, PartialEq)]
pub(crate) struct Macro {
    pub flags: u8,
    pub params: Vec<Tok>,
    pub body: Vec<Tok>,
}

pub(crate) const MATCH_CAT: u8 = 14;

/// `end_match` (§289), only where a macro is shown.
pub(crate) fn end_match() -> Tok {
    Tok::C(15 << 21)
}
pub(crate) const OUT_PARAM_CAT: u8 = 5;

fn prim_index() -> &'static std::collections::HashMap<&'static str, (u8, i32)> {
    static M: OnceLock<std::collections::HashMap<&'static str, (u8, i32)>> = OnceLock::new();
    M.get_or_init(|| {
        crate::etex::all_prims()
            .map(|&(n, c, h)| (n, (c, h)))
            .collect()
    })
}

fn prim_names() -> &'static std::collections::HashMap<(u8, i32), &'static str> {
    static M: OnceLock<std::collections::HashMap<(u8, i32), &'static str>> = OnceLock::new();
    M.get_or_init(|| {
        crate::etex::all_prims()
            .map(|&(n, c, h)| ((c, h), n))
            .collect()
    })
}

/// The primitive named `name` (frozen and internal ones included).
pub(crate) fn prim(name: &str) -> Option<(&'static str, u8, i32)> {
    let base = name.strip_prefix('\u{10FFFE}').unwrap_or(name);
    match name {
        // internal: `end_graf` from a closing brace (§1096, §1100)
        "\u{10FFFE}endgraf" => return Some(("\u{10FFFE}endgraf", cmd::PAR_END, 1)),
        "\u{10FFFE}notexpanded" => return Some(("\u{10FFFE}notexpanded", cmd::DONT_EXPAND, 0)),
        "\u{10FFFE}endv" => return Some(("\u{10FFFE}endv", cmd::IGNORE, 0)),
        // internal: `align_peek` after a row or `\noalign` (§785)
        "\u{10FFFE}alignpeek" => return Some(("\u{10FFFE}alignpeek", cmd::NO_ALIGN, 1)),
        _ => {}
    }
    prim_index().get(base).map(|&(c, h)| {
        let n = crate::etex::all_prims()
            .find(|p| p.0 == base)
            .map_or("", |p| p.0);
        (n, c, h)
    })
}

/// The name of the parameter or primitive at `(cmd, chr)`.
pub(crate) fn prim_name(c: u8, chr: i32) -> Option<&'static str> {
    prim_names().get(&(c, chr)).copied()
}

impl Mean {
    pub(crate) fn cmd_chr(&self) -> (u8, i32) {
        match self {
            Mean::Prim(_, c, h) | Mean::Reg(c, h) => (*c, *h),
            Mean::Macro(m) => (cmd::CALL + (m.flags & 3), 0),
            Mean::Char(c, h) => (*c, i32::try_from(*h).unwrap_or(0)),
            Mean::Undefined => (cmd::UNDEFINED_CS, 0),
            Mean::Font(f) => (cmd::SET_FONT, i32::try_from(*f).unwrap_or(0)),
            Mean::CharDef(n) => (cmd::CHAR_GIVEN, i32::try_from(*n).unwrap_or(0)),
            Mean::MathCharDef(n) => (cmd::MATH_GIVEN, i32::try_from(*n).unwrap_or(0)),
            Mean::NotExpanded => (cmd::RELAX, NO_EXPAND_FLAG),
        }
    }

    pub(crate) fn encode(&self, v: &Values) -> ValueId {
        let s = |x: &str| v.str(x);
        let i = |x: i64| v.int(x);
        match self {
            Mean::Prim(n, _, _) => v.list(vec![s("prim"), s(n)]),
            Mean::Macro(m) => v.list(vec![
                s("macro"),
                i(i64::from(m.flags)),
                toks_value(v, &m.params),
                toks_value(v, &m.body),
            ]),
            Mean::Char(c, h) => v.list(vec![s("char"), i(i64::from(*c)), i(i64::from(*h))]),
            Mean::Undefined => v.list(vec![s("undefined")]),
            Mean::Font(f) => v.list(vec![s("font"), i(*f)]),
            Mean::CharDef(n) => v.list(vec![s("chardef"), i(*n)]),
            Mean::MathCharDef(n) => v.list(vec![s("mathchardef"), i(*n)]),
            Mean::Reg(c, h) => {
                let (tag, base) = reg_tag(*c);
                v.list(vec![s(tag), i(i64::from(*h - base))])
            }
            Mean::NotExpanded => v.list(vec![s("notexpanded")]),
        }
    }

    pub(crate) fn decode(v: &Values, id: Option<ValueId>) -> Mean {
        let Some(id) = id else { return Mean::Undefined };
        decoded(v, id, |d| &mut d.means, || Mean::decode_new(v, id))
    }

    fn decode_new(v: &Values, id: ValueId) -> Mean {
        let xs = match &*v.get(id) {
            Value::List(xs) => xs.clone(),
            _ => return Mean::Undefined,
        };
        let Some(&t) = xs.first() else {
            return Mean::Undefined;
        };
        let tag = str_of(v, t);
        let n = |k: usize| xs.get(k).map_or(0, |&x| int_of(v, x));
        match tag.as_str() {
            "prim" => {
                let name = xs.get(1).map(|&x| str_of(v, x)).unwrap_or_default();
                prim(&name).map_or(Mean::Undefined, |(n, c, h)| Mean::Prim(n, c, h))
            }
            "macro" => Mean::Macro(Rc::new(Macro {
                flags: u8::try_from(n(1)).unwrap_or(0),
                params: toks_of(v, xs.get(2).copied()),
                body: toks_of(v, xs.get(3).copied()),
            })),
            "char" => Mean::Char(
                u8::try_from(n(1)).unwrap_or(0),
                u32::try_from(n(2)).unwrap_or(0),
            ),
            "font" => Mean::Font(n(1)),
            "chardef" => Mean::CharDef(n(1)),
            "mathchardef" => Mean::MathCharDef(n(1)),
            "countdef" => Mean::Reg(
                cmd::ASSIGN_INT,
                COUNT_BASE + i32::try_from(n(1)).unwrap_or(0),
            ),
            "dimendef" => Mean::Reg(
                cmd::ASSIGN_DIMEN,
                SCALED_BASE + i32::try_from(n(1)).unwrap_or(0),
            ),
            "skipdef" => Mean::Reg(
                cmd::ASSIGN_GLUE,
                SKIP_BASE + i32::try_from(n(1)).unwrap_or(0),
            ),
            "muskipdef" => Mean::Reg(
                cmd::ASSIGN_MU_GLUE,
                MU_SKIP_BASE + i32::try_from(n(1)).unwrap_or(0),
            ),
            "toksdef" => Mean::Reg(
                cmd::ASSIGN_TOKS,
                TOKS_BASE + i32::try_from(n(1)).unwrap_or(0),
            ),
            "notexpanded" => Mean::NotExpanded,
            _ => Mean::Undefined,
        }
    }
}

fn reg_tag(c: u8) -> (&'static str, i32) {
    match c {
        cmd::ASSIGN_INT => ("countdef", COUNT_BASE),
        cmd::ASSIGN_DIMEN => ("dimendef", SCALED_BASE),
        cmd::ASSIGN_GLUE => ("skipdef", SKIP_BASE),
        cmd::ASSIGN_MU_GLUE => ("muskipdef", MU_SKIP_BASE),
        _ => ("toksdef", TOKS_BASE),
    }
}

/// The field of the eqtb quantity at `(cmd, chr)` for the assignment
/// commands (§1224, §1228): parameters by name, registers by number.
pub(crate) fn quantity_field(c: u8, chr: i32) -> String {
    let (base, reg) = match c {
        cmd::ASSIGN_INT => (COUNT_BASE, "count"),
        cmd::ASSIGN_DIMEN => (SCALED_BASE, "dimen"),
        cmd::ASSIGN_GLUE => (SKIP_BASE, "skip"),
        cmd::ASSIGN_MU_GLUE => (MU_SKIP_BASE, "muskip"),
        _ => (TOKS_BASE, "toks"),
    };
    if chr >= base {
        format!("{reg}:{}", chr - base)
    } else {
        format!("param:{}", prim_name(c, chr).unwrap_or("?"))
    }
}

/// A glue value: `[width, stretch, stretch order, shrink, shrink order]`.
pub(crate) fn glue_value(v: &Values, g: &Glue) -> ValueId {
    v.list(vec![
        v.int(i64::from(g.width)),
        v.int(i64::from(g.stretch)),
        v.int(g.stretch_order as i64),
        v.int(i64::from(g.shrink)),
        v.int(g.shrink_order as i64),
    ])
}

pub(crate) fn order(i: i64) -> Order {
    match i {
        1 => Order::Fil,
        2 => Order::Fill,
        3 => Order::Filll,
        _ => Order::Normal,
    }
}

pub(crate) fn glue_of(v: &Values, id: Option<ValueId>) -> Glue {
    let Some(id) = id else { return Glue::ZERO };
    decoded(v, id, |d| &mut d.glues, || glue_new(v, id))
}

fn glue_new(v: &Values, id: ValueId) -> Glue {
    match &*v.get(id) {
        Value::List(xs) if xs.len() == 5 => {
            let n = |k: usize| i32::try_from(int_of(v, xs[k])).unwrap_or(0);
            Glue {
                width: n(0),
                stretch: n(1),
                stretch_order: order(int_of(v, xs[2])),
                shrink: n(3),
                shrink_order: order(int_of(v, xs[4])),
            }
        }
        _ => Glue::ZERO,
    }
}

pub(crate) fn i32_of(v: &Values, id: Option<ValueId>) -> i32 {
    id.and_then(|x| v.as_int(x)).map_or(0, |i| {
        i32::try_from(i).unwrap_or(if i < 0 { i32::MIN } else { i32::MAX })
    })
}

/// INITEX's state (§222–§240) that differs from zero, void, empty and
/// undefined: the category codes, the other code tables, the parameters,
/// and the frozen control sequences.
pub(crate) fn initex(v: &Values) -> Vec<(String, ValueId)> {
    let mut out = Vec::new();
    for k in 0..256i64 {
        let c = u8::try_from(k).unwrap_or(0);
        // §232
        let cat = match c {
            b'\\' => 0,
            b'%' => 14,
            b'a'..=b'z' | b'A'..=b'Z' => 11,
            127 => 15,
            0 => 9,
            13 => 5,
            b' ' => 10,
            _ => 12,
        };
        out.push((format!("cat:{k}"), v.int(cat)));
        let (lc, uc, sf) = match c {
            b'a'..=b'z' => (k, k - 32, 1000),
            b'A'..=b'Z' => (k + 32, k, 999),
            _ => (0, 0, 1000),
        };
        if lc != 0 {
            out.push((format!("lc:{k}"), v.int(lc)));
            out.push((format!("uc:{k}"), v.int(uc)));
        }
        out.push((format!("sf:{k}"), v.int(sf)));
        let mc = match c {
            b'0'..=b'9' => k + 0x7000,
            b'a'..=b'z' | b'A'..=b'Z' => k + 0x7100,
            _ => k,
        };
        out.push((format!("mc:{k}"), v.int(mc)));
        out.push((format!("dc:{k}"), v.int(if c == b'.' { 0 } else { -1 })));
    }
    for (p, x) in [
        ("tolerance", 10000),
        ("mag", 1000),
        ("maxdeadcycles", 25),
        ("escapechar", 92),
        ("endlinechar", 13),
        ("hangafter", 1),
    ] {
        out.push((format!("param:{p}"), v.int(x)));
    }
    let relax = Mean::Prim("relax", cmd::RELAX, 256).encode(v);
    out.push(("cs:\u{10FFFE}relax".to_owned(), relax));
    out.push((
        "cs:\u{10FFFE}endgraf".to_owned(),
        v.list(vec![v.str("prim"), v.str("\u{10FFFE}endgraf")]),
    ));
    for n in ["\u{10FFFE}alignpeek", "\u{10FFFE}cr"] {
        out.push((format!("cs:{n}"), v.list(vec![v.str("prim"), v.str(n)])));
    }
    out.extend(crate::pdfprim::initex(v));
    for &(n, _, _) in crate::etex::all_prims() {
        out.push((format!("cs:{n}"), v.list(vec![v.str("prim"), v.str(n)])));
    }
    // §552: the null font's name
    out.push((
        "fontinfo:0".to_owned(),
        v.list(vec![v.str("nullfont"), v.int(0), v.int(0)]),
    ));
    out
}
