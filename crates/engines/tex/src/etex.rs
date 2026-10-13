//! e-TeX's primitives (`etex.ch`, as pdfTeX carries them: pdfTeX
//! §1649–§1880): expressions, the new conditionals and their stack, the
//! token-list conversions, pseudo files, mark classes, the penalties
//! arrays, the interaction mode, `\protected`, TeX--XeT's state and its
//! nodes, the registers widened to 32768.
//!
//! The commands keep tex.web's command codes; their modifiers are the
//! ones below, past the `TeX82` ones, so every existing switch can hand
//! them here with one test. What a primitive keeps between commands is a
//! field: the condition stack (`ifstack`), the mark classes
//! (`marks:<kind>:<n>`, the classes used in `markclasses`), the arrays
//! (`param:<name>`), the interaction mode (`interaction`).

use std::cell::Cell;
use std::rc::Rc;

use phitex_defuse::ValueId;
use phitex_typeset::arith::{MAX_DIMEN, fract, mult_integers, nx_plus_y};
use phitex_typeset::glue::{Glue, Order};

use crate::build::Mode;
use crate::expand::str_toks;
use crate::fields::Par;
use crate::nodes;
use crate::run::{Frame, Halt, R, Run, Src};
use crate::scan::IVal;
use crate::val::{Macro, Mean, Tok, cmd, int_of, list_of, toks_of, toks_value};

// ------------------------------------------------------------- the codes

/// `last_item` modifiers (pdfTeX §1650 ff.): integers, dimensions, glue,
/// muglue, expressions.
pub(crate) mod li {
    pub const LAST_NODE_TYPE: i32 = 100;
    pub const ETEX_VERSION: i32 = 101;
    pub const CURRENT_GROUP_LEVEL: i32 = 102;
    pub const CURRENT_GROUP_TYPE: i32 = 103;
    pub const CURRENT_IF_LEVEL: i32 = 104;
    pub const CURRENT_IF_TYPE: i32 = 105;
    pub const CURRENT_IF_BRANCH: i32 = 106;
    pub const GLUE_STRETCH_ORDER: i32 = 107;
    pub const GLUE_SHRINK_ORDER: i32 = 108;
    pub const FONT_CHAR_WD: i32 = 110;
    pub const FONT_CHAR_HT: i32 = 111;
    pub const FONT_CHAR_DP: i32 = 112;
    pub const FONT_CHAR_IC: i32 = 113;
    pub const PAR_SHAPE_LENGTH: i32 = 114;
    pub const PAR_SHAPE_INDENT: i32 = 115;
    pub const PAR_SHAPE_DIMEN: i32 = 116;
    pub const GLUE_STRETCH: i32 = 117;
    pub const GLUE_SHRINK: i32 = 118;
    pub const MU_TO_GLUE: i32 = 120;
    pub const GLUE_TO_MU: i32 = 121;
    pub const NUM_EXPR: i32 = 130;
    pub const DIM_EXPR: i32 = 131;
    pub const GLUE_EXPR: i32 = 132;
    pub const MU_EXPR: i32 = 133;
}

/// `if_test` modifiers past `\ifcase` (16), and `unless_code` (pdfTeX
/// §1758).
pub(crate) const IF_DEF: i32 = 17;
pub(crate) const IF_CS: i32 = 18;
pub(crate) const IF_FONT_CHAR: i32 = 19;
pub(crate) const IF_IN_CSNAME: i32 = 20;
pub(crate) const IF_PDFABS_NUM: i32 = 21;
pub(crate) const IF_PDFABS_DIM: i32 = 22;
pub(crate) const IF_PDFPRIMITIVE: i32 = 23;
pub(crate) const UNLESS_CODE: i32 = 32;

/// `\marks` and the mark-class reads (`marks_code`, pdfTeX §1815).
pub(crate) const MARKS_CODE: i32 = 5;
/// `\middle` (`middle_noad`, pdfTeX §1810).
pub(crate) const MIDDLE_NOAD: i32 = 1;
/// TeX--XeT's `\beginL`, `\endL`, `\beginR`, `\endR` (pdfTeX §1712).
pub(crate) const BEGIN_L: i32 = 6;
pub(crate) const END_L: i32 = 7;
pub(crate) const BEGIN_R: i32 = 10;
pub(crate) const END_R: i32 = 11;

/// Parameters: modifiers past every `TeX82` one of their command.
const P: i32 = 1000;

/// The e-TeX primitives (pdfTeX §1650–§1880 `primitive` calls).
pub(crate) const PRIMS: &[(&str, u8, i32)] = &[
    ("lastnodetype", cmd::LAST_ITEM, li::LAST_NODE_TYPE),
    ("eTeXversion", cmd::LAST_ITEM, li::ETEX_VERSION),
    ("eTeXrevision", cmd::CONVERT, 100),
    ("everyeof", cmd::ASSIGN_TOKS, P),
    ("tracingassigns", cmd::ASSIGN_INT, P),
    ("tracinggroups", cmd::ASSIGN_INT, P + 1),
    ("tracingifs", cmd::ASSIGN_INT, P + 2),
    ("tracingscantokens", cmd::ASSIGN_INT, P + 3),
    ("tracingnesting", cmd::ASSIGN_INT, P + 4),
    ("predisplaydirection", cmd::ASSIGN_INT, P + 5),
    ("lastlinefit", cmd::ASSIGN_INT, P + 6),
    ("savingvdiscards", cmd::ASSIGN_INT, P + 7),
    ("savinghyphcodes", cmd::ASSIGN_INT, P + 8),
    ("ignoreprimitiveerror", cmd::ASSIGN_INT, P + 9),
    ("TeXXeTstate", cmd::ASSIGN_INT, P + 10),
    ("currentgrouplevel", cmd::LAST_ITEM, li::CURRENT_GROUP_LEVEL),
    ("currentgrouptype", cmd::LAST_ITEM, li::CURRENT_GROUP_TYPE),
    ("currentiflevel", cmd::LAST_ITEM, li::CURRENT_IF_LEVEL),
    ("currentiftype", cmd::LAST_ITEM, li::CURRENT_IF_TYPE),
    ("currentifbranch", cmd::LAST_ITEM, li::CURRENT_IF_BRANCH),
    ("fontcharwd", cmd::LAST_ITEM, li::FONT_CHAR_WD),
    ("fontcharht", cmd::LAST_ITEM, li::FONT_CHAR_HT),
    ("fontchardp", cmd::LAST_ITEM, li::FONT_CHAR_DP),
    ("fontcharic", cmd::LAST_ITEM, li::FONT_CHAR_IC),
    ("parshapelength", cmd::LAST_ITEM, li::PAR_SHAPE_LENGTH),
    ("parshapeindent", cmd::LAST_ITEM, li::PAR_SHAPE_INDENT),
    ("parshapedimen", cmd::LAST_ITEM, li::PAR_SHAPE_DIMEN),
    ("showgroups", cmd::XRAY, 4),
    ("showtokens", cmd::XRAY, 5),
    ("unexpanded", cmd::THE, 1),
    ("detokenize", cmd::THE, 5),
    ("showifs", cmd::XRAY, 6),
    ("interactionmode", cmd::SET_PAGE_INT, 2),
    ("middle", cmd::LEFT_RIGHT, MIDDLE_NOAD),
    ("beginL", cmd::VALIGN, BEGIN_L),
    ("endL", cmd::VALIGN, END_L),
    ("beginR", cmd::VALIGN, BEGIN_R),
    ("endR", cmd::VALIGN, END_R),
    ("scantokens", cmd::INPUT, 2),
    ("readline", cmd::READ_TO_CS, 1),
    ("unless", cmd::EXPAND_AFTER, 1),
    ("ifdefined", cmd::IF_TEST, IF_DEF),
    ("ifcsname", cmd::IF_TEST, IF_CS),
    ("iffontchar", cmd::IF_TEST, IF_FONT_CHAR),
    ("ifincsname", cmd::IF_TEST, IF_IN_CSNAME),
    ("protected", cmd::PREFIX, 8),
    ("numexpr", cmd::LAST_ITEM, li::NUM_EXPR),
    ("dimexpr", cmd::LAST_ITEM, li::DIM_EXPR),
    ("glueexpr", cmd::LAST_ITEM, li::GLUE_EXPR),
    ("muexpr", cmd::LAST_ITEM, li::MU_EXPR),
    ("gluestretchorder", cmd::LAST_ITEM, li::GLUE_STRETCH_ORDER),
    ("glueshrinkorder", cmd::LAST_ITEM, li::GLUE_SHRINK_ORDER),
    ("gluestretch", cmd::LAST_ITEM, li::GLUE_STRETCH),
    ("glueshrink", cmd::LAST_ITEM, li::GLUE_SHRINK),
    ("mutoglue", cmd::LAST_ITEM, li::MU_TO_GLUE),
    ("gluetomu", cmd::LAST_ITEM, li::GLUE_TO_MU),
    ("marks", cmd::MARK, MARKS_CODE),
    ("topmarks", cmd::TOP_BOT_MARK, MARKS_CODE),
    ("firstmarks", cmd::TOP_BOT_MARK, MARKS_CODE + 1),
    ("botmarks", cmd::TOP_BOT_MARK, MARKS_CODE + 2),
    ("splitfirstmarks", cmd::TOP_BOT_MARK, MARKS_CODE + 3),
    ("splitbotmarks", cmd::TOP_BOT_MARK, MARKS_CODE + 4),
    ("pagediscards", cmd::UN_VBOX, 2),
    ("splitdiscards", cmd::UN_VBOX, 3),
    ("interlinepenalties", cmd::SET_SHAPE, 1),
    ("clubpenalties", cmd::SET_SHAPE, 2),
    ("widowpenalties", cmd::SET_SHAPE, 3),
    ("displaywidowpenalties", cmd::SET_SHAPE, 4),
    // web2c's change files (`partoken.ch`, `showstream.ch`)
    ("tracingstacklevels", cmd::ASSIGN_INT, P + 11),
    ("partokencontext", cmd::ASSIGN_INT, P + 12),
    ("showstream", cmd::ASSIGN_INT, P + 13),
    ("partokenname", cmd::SET_INTERACTION, 10),
];

/// Every primitive: `TeX82`'s, e-TeX's, pdfTeX's.
pub(crate) fn all_prims() -> impl Iterator<Item = &'static (&'static str, u8, i32)> {
    crate::prims::PRIMS
        .iter()
        .chain(PRIMS)
        .chain(crate::pdfprim::PRIMS)
}

const SHAPES: [&str; 5] = [
    "parshape",
    "param:interlinepenalties",
    "param:clubpenalties",
    "param:widowpenalties",
    "param:displaywidowpenalties",
];

const MARK_KINDS: [&str; 5] = ["top", "first", "bot", "splitfirst", "splitbot"];

/// `max_reg_num` (pdfTeX §1809).
pub(crate) const MAX_REG_NUM: i32 = 32767;

// --------------------------------------------------------- `\ifincsname`

thread_local! {
    /// `is_in_csname` (pdfTeX §1764): `\csname`s and `\ifcsname`s being
    /// scanned in this run (a run is sequential, the count goes back to
    /// what it was when the scan ends or is abandoned).
    static IN_CSNAME: Cell<u32> = const { Cell::new(0) };
}

/// While a `\csname` is scanned.
pub(crate) struct InCsname;

impl InCsname {
    pub(crate) fn enter() -> InCsname {
        IN_CSNAME.with(|c| c.set(c.get() + 1));
        InCsname
    }
}

impl Drop for InCsname {
    fn drop(&mut self) {
        IN_CSNAME.with(|c| c.set(c.get().saturating_sub(1)));
    }
}

// --------------------------------------------------------- `\protected`

/// The macro flag of `\protected` (pdfTeX §1786: `protected_token`).
pub(crate) const PROTECTED: u8 = 4;

/// §1213 (e-TeX): `\protected` taken out of the prefixes.
pub(crate) fn take_protected(a: &mut i32) -> bool {
    if *a >= 8 {
        *a -= 8;
        true
    } else {
        false
    }
}

/// pdfTeX §478 (e-TeX): a protected macro is not expanded while a token
/// list is built with expansion; true if `cur` was one (it is now
/// `\relax`-like, `no_expand_flag`).
pub(crate) fn protect(cur: &mut crate::run::Cur) -> bool {
    if matches!(&cur.mean, Mean::Macro(x) if x.flags & PROTECTED != 0) {
        cur.cmd = cmd::RELAX;
        cur.chr = crate::val::NO_EXPAND_FLAG;
        return true;
    }
    false
}

/// Whether `(c, m)` is an internal quantity of e-TeX or pdfTeX under a
/// `TeX82` command (`\interactionmode`, the penalties arrays, the font
/// codes).
pub(crate) fn is_internal(c: u8, m: i32) -> bool {
    match c {
        cmd::SET_PAGE_INT => m == 2,
        cmd::SET_SHAPE => m > 0,
        cmd::ASSIGN_FONT_INT => m >= 2,
        _ => false,
    }
}

// ------------------------------------------------------------- the hooks

impl Run<'_, '_> {
    /// `scan_register_num` (pdfTeX §1809): 0..32767.
    pub(crate) fn scan_register_num(&mut self) -> R<i32> {
        let n = self.scan_int()?;
        if (0..=MAX_REG_NUM).contains(&n) {
            Ok(n)
        } else {
            self.log("error", "Bad register code")?;
            Ok(0)
        }
    }
}

/// The expandable e-TeX and pdfTeX commands `expand` (§366) does not
/// know; true if `cur` was one (and was expanded).
pub(crate) fn expand(r: &mut Run<'_, '_>) -> R<bool> {
    let (c, chr) = (r.cur.cmd, r.cur.chr);
    match c {
        cmd::EXPAND_AFTER if chr == 1 => unless(r)?,
        cmd::NO_EXPAND if chr == 1 => crate::pdfprim::pdf_primitive(r)?,
        cmd::INPUT if chr == 2 => pseudo_start(r)?,
        cmd::TOP_BOT_MARK if chr >= MARKS_CODE => {
            // pdfTeX §386 (e-TeX)
            let n = r.scan_register_num()?;
            let kind = MARK_KINDS[usize::try_from(chr - MARKS_CODE).unwrap_or(0).min(4)];
            let m = mark_read(r, kind, n)?;
            let toks = toks_of(r.v, m.filter(|&x| x != ValueId::UNIT));
            r.push_toks(toks);
        }
        cmd::CONVERT if chr == 100 => r.push_toks(str_toks(".6")),
        cmd::CONVERT if chr > 100 => crate::pdfprim::convert(r, chr)?,
        _ => return Ok(false),
    }
    Ok(true)
}

/// After `expand` did a conditional or `\fi`: nothing. A conditional
/// is never split across nodes (it runs to its `\fi` in one run), so
/// the condition stack is the run's own `conds` (§489) and no record
/// leaves the run.
pub(crate) fn after_expand(_r: &mut Run<'_, '_>, _c: u8) {}

/// `\unless` (pdfTeX §1758).
fn unless(r: &mut Run<'_, '_>) -> R<()> {
    r.get_token()?;
    if r.cur.cmd == cmd::IF_TEST && r.cur.chr != 16 {
        r.cur.chr += UNLESS_CODE;
        r.conditional()?;
        return Ok(());
    }
    r.log("error", "You can't use `\\unless' before this")?;
    r.back_cur();
    Ok(())
}

// --------------------------------------------------- the condition stack

/// A record of the condition stack (`cond_ptr`, §489): the
/// conditional's code (`cur_if`, with `unless_code`) and its `if_limit`.
#[derive(Clone, Debug)]
struct IfRec {
    code: i32,
    limit: i32,
}

/// The whole stack (`\showifs`, `\currentif…`): the run's own
/// conditionals, since a conditional never spans nodes.
fn if_stack(r: &Run<'_, '_>) -> Vec<IfRec> {
    r.conds
        .iter()
        .zip(&r.if_codes)
        .map(|(&l, &code)| IfRec {
            code,
            limit: i32::from(l),
        })
        .collect()
}

/// The conditional `code` (with `unless_code`) begins (`push_cond`,
/// §495): its limit is on `conds`, kept by `conditional`, its code on
/// `if_codes`.
pub(crate) fn if_begin(r: &mut Run<'_, '_>, code: i32) {
    r.if_codes.push(code);
}

/// The tests e-TeX and pdfTeX add to `conditional` (§501).
pub(crate) fn if_test(r: &mut Run<'_, '_>, this_if: i32) -> R<bool> {
    Ok(match this_if {
        IF_DEF => {
            // pdfTeX §1759
            if !r.get_next()? {
                return Ok(false);
            }
            r.cur.cmd != cmd::UNDEFINED_CS
        }
        IF_CS => {
            // pdfTeX §1760
            let _g = InCsname::enter();
            let mut name = String::new();
            loop {
                r.get_x_token()?;
                if r.cur.is_cs() {
                    break;
                }
                crate::print::push_code(&mut name, r.cur.tok.chr() & 0xFF);
            }
            if r.cur.cmd != cmd::END_CS_NAME {
                // "Missing \endcsname inserted"
                r.log("error", "Missing \\endcsname inserted")?;
                r.back_cur();
            }
            let t = Tok::S(Rc::from(name));
            r.mean_of(&t)? != Mean::Undefined
        }
        IF_FONT_CHAR => {
            // pdfTeX §1762
            let f = r.scan_font_ident()?;
            let c = r.scan_char_num()?;
            char_dims(r, f, c)?.is_some()
        }
        IF_IN_CSNAME => IN_CSNAME.with(|c| c.get() > 0),
        IF_PDFABS_NUM | IF_PDFABS_DIM => {
            let n = if this_if == IF_PDFABS_NUM {
                r.scan_int()?
            } else {
                r.scan_normal_dimen()?
            };
            let n = n.saturating_abs();
            r.skip_blanks_x()?;
            let rel = if !r.cur.is_cs()
                && r.cur.tok.cat() == Some(12)
                && (60..=62).contains(&r.cur.tok.chr())
            {
                r.cur.tok.chr()
            } else {
                r.log("error", "Missing = inserted for \\ifpdfabsnum")?;
                r.back_cur();
                61
            };
            let x = if this_if == IF_PDFABS_NUM {
                r.scan_int()?
            } else {
                r.scan_normal_dimen()?
            };
            let x = x.saturating_abs();
            match rel {
                60 => n < x,
                61 => n == x,
                _ => n > x,
            }
        }
        IF_PDFPRIMITIVE => crate::pdfprim::if_pdf_primitive(r)?,
        _ => false,
    })
}

// ------------------------------------------------------- `last_item`

/// The `last_item` values e-TeX and pdfTeX add (pdfTeX §424 ff.).
#[allow(clippy::too_many_lines)]
pub(crate) fn last_item(r: &mut Run<'_, '_>, m: i32) -> R<IVal> {
    let v = r.v;
    Ok(match m {
        li::LAST_NODE_TYPE => IVal::Int(last_node_type(r)?),
        li::ETEX_VERSION => IVal::Int(2),
        li::CURRENT_GROUP_LEVEL => {
            let md = r.mode()?;
            IVal::Int(i32::try_from(md.depth).unwrap_or(0))
        }
        li::CURRENT_GROUP_TYPE => {
            let md = r.mode()?;
            IVal::Int(i32::try_from(md.group).unwrap_or(0))
        }
        li::CURRENT_IF_LEVEL | li::CURRENT_IF_TYPE | li::CURRENT_IF_BRANCH => {
            let st = if_stack(r);
            let Some(top) = st.last() else {
                return Ok(IVal::Int(0));
            };
            IVal::Int(match m {
                li::CURRENT_IF_LEVEL => i32::try_from(st.len()).unwrap_or(0),
                li::CURRENT_IF_TYPE => {
                    if top.code < 0 {
                        0
                    } else if top.code < UNLESS_CODE {
                        top.code + 1
                    } else {
                        -(top.code - UNLESS_CODE + 1)
                    }
                }
                _ => match top.limit {
                    3 | 4 => 1,
                    2 => -1,
                    _ => 0,
                },
            })
        }
        li::GLUE_STRETCH_ORDER | li::GLUE_SHRINK_ORDER => {
            let g = r.scan_glue(2)?;
            let o = if m == li::GLUE_STRETCH_ORDER {
                g.stretch_order
            } else {
                g.shrink_order
            };
            IVal::Int(order_num(o))
        }
        li::FONT_CHAR_WD | li::FONT_CHAR_HT | li::FONT_CHAR_DP | li::FONT_CHAR_IC => {
            let f = r.scan_font_ident()?;
            let c = r.scan_char_num()?;
            IVal::Dimen(char_dims(r, f, c)?.map_or(0, |d| match m {
                li::FONT_CHAR_WD => d.width,
                li::FONT_CHAR_HT => d.height,
                li::FONT_CHAR_DP => d.depth,
                _ => d.italic,
            }))
        }
        li::PAR_SHAPE_LENGTH | li::PAR_SHAPE_INDENT | li::PAR_SHAPE_DIMEN => {
            // pdfTeX §1702
            let mut q = m - li::PAR_SHAPE_LENGTH;
            let mut n = r.scan_int()?;
            let shape = r
                .get("parshape")?
                .map(|x| list_of(v, x))
                .unwrap_or_default();
            let count = i32::try_from(shape.len() / 2).unwrap_or(0);
            IVal::Dimen(if count == 0 || n <= 0 {
                0
            } else {
                if q == 2 {
                    q = n % 2;
                    n = i32::midpoint(n, q);
                }
                n = n.min(count);
                // mem[par_shape_ptr+2n-q]: indent at 2n-1, length at 2n
                let k = usize::try_from(2 * n - q - 1).unwrap_or(0);
                shape
                    .get(k)
                    .map_or(0, |&x| i32::try_from(int_of(v, x)).unwrap_or(0))
            })
        }
        li::GLUE_STRETCH | li::GLUE_SHRINK => {
            let g = r.scan_glue(2)?;
            IVal::Dimen(if m == li::GLUE_STRETCH {
                g.stretch
            } else {
                g.shrink
            })
        }
        li::MU_TO_GLUE => IVal::Glue(r.scan_glue(3)?),
        li::GLUE_TO_MU => IVal::Mu(r.scan_glue(2)?),
        li::NUM_EXPR | li::DIM_EXPR | li::GLUE_EXPR | li::MU_EXPR => {
            let l = u8::try_from(m - li::NUM_EXPR).unwrap_or(0);
            scan_expr(r, l)?
        }
        _ => crate::pdfprim::last_item(r, m)?,
    })
}

fn order_num(o: Order) -> i32 {
    match o {
        Order::Normal => 0,
        Order::Fil => 1,
        Order::Fill => 2,
        Order::Filll => 3,
    }
}

/// A character's dimensions in font `f`, if it exists.
pub(crate) fn char_dims(r: &mut Run<'_, '_>, f: i64, c: i32) -> R<Option<phitex_tfm::CharDims>> {
    let Ok(fu) = u32::try_from(f) else {
        return Ok(None);
    };
    let mut view = crate::fonts::View::default();
    r.add_font(&mut view, fu)?;
    Ok(view.char_dims(fu, u32::try_from(c).unwrap_or(0)))
}

/// `\lastnodetype` (pdfTeX §424): the type of the current list's tail.
fn last_node_type(r: &mut Run<'_, '_>) -> R<i32> {
    let md = r.mode()?;
    let (items, since_bp) = r.list_items(md.lid)?;
    let v = r.v;
    let main = md.lid == 0 && md.km == 1;
    let tail = if main && since_bp == 0 {
        None
    } else {
        items.last().copied()
    };
    let Some(t) = tail else { return Ok(-1) };
    Ok(match nodes::tag(v, t) {
        nodes::T_CHAR => 0,
        nodes::T_BOX => {
            if nodes::box_is_h(v, t) {
                1
            } else {
                2
            }
        }
        x @ nodes::T_RULE..=nodes::T_WHATSIT => i32::try_from(x).unwrap_or(0) + 1,
        nodes::T_LANGUAGE => 9,
        nodes::T_MATH => 10,
        nodes::T_GLUE => 11,
        nodes::T_KERN => 12,
        nodes::T_PENALTY => 13,
        _ => 15,
    })
}

// ------------------------------------------------------- expressions

const INFINITY: i32 = 0o17777777777;
const EXPR_NONE: u8 = 0;
const EXPR_ADD: u8 = 1;
const EXPR_SUB: u8 = 2;
const EXPR_MULT: u8 = 3;
const EXPR_DIV: u8 = 4;
const EXPR_SCALE: u8 = 5;

/// `add_or_sub` (pdfTeX §1673).
fn add_or_sub(x: i32, y: i32, max: i32, negative: bool, err: &mut bool) -> i32 {
    let y = if negative { -y } else { y };
    let ok = if x >= 0 { y <= max - x } else { y >= -max - x };
    if ok {
        x + y
    } else {
        *err = true;
        0
    }
}

/// `quotient` (pdfTeX §1675).
fn quotient(n: i32, d: i32, err: &mut bool) -> i32 {
    if d == 0 {
        *err = true;
        return 0;
    }
    let (mut n, mut d) = (i64::from(n), i64::from(d));
    let mut negative = false;
    if d < 0 {
        d = -d;
        negative = true;
    }
    if n < 0 {
        n = -n;
        negative = !negative;
    }
    let mut a = n / d;
    n -= a * d;
    d = n - d;
    if d + n >= 0 {
        a += 1;
    }
    if negative {
        a = -a;
    }
    i32::try_from(a).unwrap_or(0)
}

fn normalize(g: &mut Glue) {
    if g.stretch == 0 {
        g.stretch_order = Order::Normal;
    }
    if g.shrink == 0 {
        g.shrink_order = Order::Normal;
    }
}

/// One saved level of the expression stack (pdfTeX §1669).
struct Level {
    l: u8,
    s: u8,
    r: u8,
    e: Glue,
    t: Glue,
    n: i32,
}

/// `scan_expr` (pdfTeX §1661–§1680): an expression of type `l0` (0
/// integer, 1 dimension, 2 glue, 3 muglue).
#[allow(clippy::too_many_lines)]
fn scan_expr(run: &mut Run<'_, '_>, l0: u8) -> R<IVal> {
    let zero = Glue::ZERO;
    let mut b = false;
    let mut stack: Vec<Level> = Vec::new();
    let mut l = l0;
    let (mut r, mut e, mut s, mut t, mut n);
    'restart: loop {
        r = EXPR_NONE;
        e = zero;
        s = EXPR_NONE;
        t = zero;
        n = 0;
        'cont: loop {
            let o_type = if s == EXPR_NONE { l } else { 0 };
            // §1667: a factor, or a subexpression
            run.skip_blanks_x()?;
            if run.is_other(b'(') {
                stack.push(Level { l, s, r, e, t, n });
                l = o_type;
                continue 'restart;
            }
            run.back_cur();
            let mut f = match o_type {
                0 => Glue {
                    width: run.scan_int()?,
                    ..zero
                },
                1 => Glue {
                    width: run.scan_normal_dimen()?,
                    ..zero
                },
                2 => run.scan_glue(2)?,
                _ => run.scan_glue(3)?,
            };
            'found: loop {
                // §1666: the next operator
                run.skip_blanks_x()?;
                let o = if run.is_other(b'+') {
                    EXPR_ADD
                } else if run.is_other(b'-') {
                    EXPR_SUB
                } else if run.is_other(b'*') {
                    EXPR_MULT
                } else if run.is_other(b'/') {
                    EXPR_DIV
                } else {
                    if stack.is_empty() {
                        if run.cur.cmd != cmd::RELAX {
                            run.back_cur();
                        }
                    } else if !run.is_other(b')') {
                        run.log("error", "Missing ) inserted for expression")?;
                        run.back_cur();
                    }
                    EXPR_NONE
                };
                let mut err = b;
                // §1671
                if l == 0 || s > EXPR_SUB {
                    // an integer cannot exceed `infinity` here
                } else if l == 1 {
                    if f.width.saturating_abs() > MAX_DIMEN {
                        err = true;
                        f.width = 0;
                    }
                } else if f.width.saturating_abs() > MAX_DIMEN
                    || f.stretch.saturating_abs() > MAX_DIMEN
                    || f.shrink.saturating_abs() > MAX_DIMEN
                {
                    err = true;
                    f = zero;
                }
                let mut o = o;
                // §1672 and the others: the current term
                match s {
                    EXPR_NONE => {
                        t = f;
                        if l >= 2 && o != EXPR_NONE {
                            normalize(&mut t);
                        }
                    }
                    EXPR_MULT => {
                        if o == EXPR_DIV {
                            n = f.width;
                            o = EXPR_SCALE;
                        } else if l == 0 {
                            t.width = mult_integers(t.width, f.width).unwrap_or_else(|| {
                                err = true;
                                0
                            });
                        } else {
                            let mut m = |x: i32| {
                                nx_plus_y(x, f.width, 0).unwrap_or_else(|| {
                                    err = true;
                                    0
                                })
                            };
                            t.width = m(t.width);
                            if l >= 2 {
                                t.stretch = m(t.stretch);
                                t.shrink = m(t.shrink);
                            }
                        }
                    }
                    EXPR_DIV => {
                        t.width = quotient(t.width, f.width, &mut err);
                        if l >= 2 {
                            t.stretch = quotient(t.stretch, f.width, &mut err);
                            t.shrink = quotient(t.shrink, f.width, &mut err);
                        }
                    }
                    _ => {
                        // EXPR_SCALE
                        let max = if l == 0 { INFINITY } else { MAX_DIMEN };
                        let mut sc = |x: i32| {
                            fract(x, n, f.width, max).unwrap_or_else(|| {
                                err = true;
                                0
                            })
                        };
                        t.width = sc(t.width);
                        if l >= 2 {
                            t.stretch = sc(t.stretch);
                            t.shrink = sc(t.shrink);
                        }
                    }
                }
                if o > EXPR_SUB {
                    s = o;
                } else {
                    // §1673: the current expression
                    s = EXPR_NONE;
                    if r == EXPR_NONE {
                        e = t;
                    } else {
                        let neg = r == EXPR_SUB;
                        let max = if l == 0 { INFINITY } else { MAX_DIMEN };
                        e.width = add_or_sub(e.width, t.width, max, neg, &mut err);
                        if l >= 2 {
                            // §1674
                            if e.stretch_order == t.stretch_order {
                                e.stretch =
                                    add_or_sub(e.stretch, t.stretch, MAX_DIMEN, neg, &mut err);
                            } else if e.stretch_order < t.stretch_order && t.stretch != 0 {
                                e.stretch = if neg { -t.stretch } else { t.stretch };
                                e.stretch_order = t.stretch_order;
                            }
                            if e.shrink_order == t.shrink_order {
                                e.shrink = add_or_sub(e.shrink, t.shrink, MAX_DIMEN, neg, &mut err);
                            } else if e.shrink_order < t.shrink_order && t.shrink != 0 {
                                e.shrink = if neg { -t.shrink } else { t.shrink };
                                e.shrink_order = t.shrink_order;
                            }
                            normalize(&mut e);
                        }
                    }
                    r = o;
                }
                b = err;
                if o != EXPR_NONE {
                    continue 'cont;
                }
                if let Some(lv) = stack.pop() {
                    // §1670
                    f = e;
                    e = lv.e;
                    t = lv.t;
                    n = lv.n;
                    s = lv.s;
                    r = lv.r;
                    l = lv.l;
                    continue 'found;
                }
                break 'restart;
            }
        }
    }
    if b {
        run.log("error", "Arithmetic overflow")?;
        e = zero;
    }
    Ok(match l0 {
        0 => IVal::Int(e.width),
        1 => IVal::Dimen(e.width),
        2 => IVal::Glue(e),
        _ => IVal::Mu(e),
    })
}

// --------------------------------------------- `\unexpanded`, `\detokenize`

/// `the_toks` for `\unexpanded` and `\detokenize` (pdfTeX §465).
pub(crate) fn the_toks(r: &mut Run<'_, '_>) -> R<Vec<Tok>> {
    let c = r.cur.chr;
    let (_, body) = r.scan_toks(false, false)?;
    if c == 1 {
        return Ok(body);
    }
    let mut s = String::new();
    r.show_toks_into(&mut s, &body)?;
    Ok(str_toks(&s))
}

// --------------------------------------------------------- pseudo files

/// The end-of-file marker of a pseudo file.
const EOF_PSEUDO: &str = "\u{10FFFE}eofp";

/// `\scantokens` (`pseudo_start`, pdfTeX §1766–§1771): the token list
/// shown as a string, cut into lines at `\newlinechar`, read as a file.
fn pseudo_start(r: &mut Run<'_, '_>) -> R<()> {
    let (_, body) = r.scan_toks(false, false)?;
    let mut s = String::new();
    r.show_toks_into(&mut s, &body)?;
    let nl = i64::from(r.ipar(Par::newlinechar)?);
    let mut lines: Vec<Vec<u32>> = Vec::new();
    let mut cur: Vec<u32> = Vec::new();
    let mut ended = true;
    for ch in s.chars() {
        let c = u32::from(ch);
        if i64::from(c) == nl {
            lines.push(std::mem::take(&mut cur));
            ended = true;
        } else {
            cur.push(c);
            ended = false;
        }
    }
    if !ended {
        lines.push(cur);
    }
    r.frames
        .push_front(Frame::Toks(Rc::from(vec![Tok::cs(EOF_PSEUDO)]), 0));
    for mut buf in lines.into_iter().rev() {
        // `pseudo_input`: trailing spaces go
        while buf.last() == Some(&32) {
            buf.pop();
        }
        r.frames.push_front(Frame::Src(Src {
            buf,
            loc: 0,
            state: crate::run::NEW_LINE,
            endch: -2,
            starts: false,
            ends: true,
            origin: None,
            rescanned: true,
        }));
    }
    Ok(())
}

/// The end of a file or pseudo file reached (§362, pdfTeX §362): first
/// `\everyeof`, unless `\endinput` ended it; then it closes.
pub(crate) fn end_of_file(r: &mut Run<'_, '_>, marker: &str) -> R<()> {
    let base = marker.trim_end_matches(['S', 'F']);
    let pseudo = base == EOF_PSEUDO;
    if marker == base {
        let ev = r.par(Par::everyeof)?;
        let toks = toks_of(r.v, ev);
        if !toks.is_empty() {
            r.frames
                .push_front(Frame::Toks(Rc::from(vec![Tok::cs(&format!("{base}S"))]), 0));
            r.push_toks(toks);
            return Ok(());
        }
    }
    if !pseudo {
        r.log("close", ")")?;
    }
    Ok(())
}

/// `\endinput` sets `force_eof`: the file's end inserts no `\everyeof`.
pub(crate) fn force_eof(r: &mut Run<'_, '_>) {
    for f in &mut r.frames {
        if let Frame::Toks(t, p) = f
            && let Some(k) = t[*p..].iter().position(
                |x| matches!(x, Tok::S(n) if n.starts_with("\u{10FFFE}eof") && !n.ends_with(['S', 'F'])),
            )
        {
            let mut v: Vec<Tok> = t.to_vec();
            if let Tok::S(n) = &v[*p + k] {
                v[*p + k] = Tok::cs(&format!("{n}F"));
            }
            *t = Rc::from(v);
            return;
        }
    }
}

// ------------------------------------------------------- `\readline`

/// `\readline` (pdfTeX §1773): the next line of stream `n`, its
/// characters as category 12 (spaces 10).
fn read_line(r: &mut Run<'_, '_>, global: bool) -> R<()> {
    let n = r.scan_int()?;
    if !r.scan_keyword("to")? {
        r.log("error", "Missing `to' inserted")?;
    }
    let p = get_r_token(r)?;
    let m = if (0..=15).contains(&n) { n } else { 16 };
    let field = format!("read:{m}");
    let st = r.get(&field)?.filter(|&x| x != ValueId::UNIT);
    let mut body = Vec::new();
    if let Some(st) = st {
        // §486: at the file's end, an empty line, and the stream closes
        let text = r.next_read_line(&field, st).unwrap_or_default();
        let e = i64::from(r.ipar(Par::endlinechar)?);
        if let Frame::Src(s) = crate::run::src_frame(&text, e) {
            for c in s.buf {
                body.push(if c == 32 {
                    crate::val::space_tok()
                } else {
                    Tok::ch(12, c)
                });
            }
        }
    } else {
        r.log(
            "error",
            "*** (cannot \\read from terminal in nonstop modes)",
        )?;
    }
    let m = Mean::Macro(Rc::new(Macro {
        flags: 0,
        params: Vec::new(),
        body,
    }));
    r.define_cs(&p, &m, global);
    Ok(())
}

/// `get_r_token` (§1215).
fn get_r_token(r: &mut Run<'_, '_>) -> R<Tok> {
    loop {
        r.get_token()?;
        if r.cur.tok != crate::val::space_tok() {
            break;
        }
    }
    if !r.cur.is_cs() {
        r.back_cur();
        return Ok(Tok::cs("inaccessible"));
    }
    Ok(r.cur.tok.clone())
}

// --------------------------------------------------------- assignments

/// The assignments e-TeX and pdfTeX add to `prefixed_command`; true if
/// `cur` was one.
pub(crate) fn prefixed(r: &mut Run<'_, '_>, global: bool) -> R<bool> {
    let v = r.v;
    let (c, chr) = (r.cur.cmd, r.cur.chr);
    match c {
        cmd::READ_TO_CS if chr == 1 => read_line(r, global)?,
        cmd::SET_SHAPE if chr > 0 => {
            // pdfTeX §1248
            r.scan_optional_equals()?;
            let n = r.scan_int()?;
            let mut xs = Vec::new();
            for _ in 0..n.max(0) {
                xs.push(v.int(i64::from(r.scan_int()?)));
            }
            let name = SHAPES[usize::try_from(chr).unwrap_or(1).min(4)];
            r.set(name, v.list(xs), global);
        }
        cmd::SET_PAGE_INT if chr == 2 => {
            // pdfTeX §1246: \interactionmode
            r.scan_optional_equals()?;
            let n = r.scan_int()?;
            if (0..=3).contains(&n) {
                set_interaction(r, n)?;
            } else {
                r.log("error", "Bad interaction mode")?;
            }
        }
        cmd::SET_INTERACTION if chr <= 3 => set_interaction(r, chr)?,
        cmd::SET_INTERACTION if chr == 10 => {
            // partoken.ch: `\partokenname`
            let t = get_r_token(r)?;
            r.set("partoken", t.encode(v), true);
        }
        cmd::SET_INTERACTION | cmd::ASSIGN_FONT_INT => {
            return crate::pdfprim::prefixed(r, global);
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// `new_interaction` (§1265).
fn set_interaction(r: &mut Run<'_, '_>, n: i32) -> R<()> {
    r.log("println", "")?;
    let x = r.v.int(i64::from(n));
    r.set("interaction", x, true);
    Ok(())
}

/// The internal quantities of e-TeX's `set_shape` and `set_page_int`
/// codes (pdfTeX §423, §419).
pub(crate) fn internal(r: &mut Run<'_, '_>) -> R<IVal> {
    let (c, m) = (r.cur.cmd, r.cur.chr);
    let v = r.v;
    if c == cmd::SET_PAGE_INT {
        return Ok(IVal::Int(r.int32("interaction")?));
    }
    if c == cmd::ASSIGN_FONT_INT {
        return crate::pdfprim::font_code(r, m);
    }
    // a penalties array: the count at 0
    let n = r.scan_int()?;
    let name = SHAPES[usize::try_from(m).unwrap_or(1).min(4)];
    let xs = r.get(name)?.map(|x| list_of(v, x)).unwrap_or_default();
    let count = i32::try_from(xs.len()).unwrap_or(0);
    Ok(IVal::Int(if xs.is_empty() || n < 0 {
        0
    } else if n == 0 {
        count
    } else {
        let k = usize::try_from(n.min(count) - 1).unwrap_or(0);
        i32::try_from(int_of(v, xs[k])).unwrap_or(0)
    }))
}

/// A penalties array for the line breaker.
pub(crate) fn penalties(r: &mut Run<'_, '_>, k: usize) -> R<Vec<i32>> {
    let v = r.v;
    Ok(r.get(SHAPES[k])?
        .map(|x| list_of(v, x))
        .unwrap_or_default()
        .iter()
        .map(|&x| i32::try_from(int_of(v, x)).unwrap_or(0))
        .collect())
}

/// e-TeX's part of `normal_paragraph` (pdfTeX §1070).
pub(crate) fn normal_paragraph(r: &mut Run<'_, '_>) -> R<()> {
    let v = r.v;
    if r.get(SHAPES[1])?.is_some_and(|x| !list_of(v, x).is_empty()) {
        r.set(SHAPES[1], v.list(Vec::new()), false);
    }
    Ok(())
}

// --------------------------------------------------- the main control

/// The commands of `main_control` e-TeX and pdfTeX add (or change):
/// true if `cur` was one.
pub(crate) fn command(r: &mut Run<'_, '_>, md: &Mode) -> R<bool> {
    let (c, chr) = (r.cur.cmd, r.cur.chr);
    let am = md.m.abs();
    match c {
        cmd::MARK if chr == MARKS_CODE => marks(r, md)?,
        cmd::VALIGN if chr > 0 => {
            if am == 1 {
                r.back_cur();
                r.new_graf(md, true)?;
            } else if am == 102 {
                // pdfTeX §1130: TeX--XeT's math nodes
                if r.param("TeXXeTstate")? > 0 {
                    let v = r.v;
                    let after = chr == END_L || chr == END_R;
                    let node = v.list(vec![
                        v.int(nodes::T_MATH),
                        v.int(i64::from(after)),
                        v.int(0),
                    ]);
                    r.append(md.lid, node);
                }
            } else {
                r.insert_dollar_sign()?;
            }
        }
        cmd::UN_VBOX if chr >= 2 => {
            // pdfTeX §1110: \pagediscards, \splitdiscards
            if am == 1 {
                let name = if chr == 2 { "disc:page" } else { "disc:split" };
                let items = r.get(name)?.map(|x| list_of(r.v, x)).unwrap_or_default();
                for x in items {
                    r.append(md.lid, x);
                }
                r.set(name, r.v.list(Vec::new()), true);
            } else if am == 102 {
                r.head_for_vmode(md);
            } else {
                r.insert_dollar_sign()?;
            }
        }
        cmd::START_PAR if chr == 2 && am != 1 => {
            // pdfTeX §1093: \quitvmode in horizontal and math mode
        }
        cmd::LEFT_RIGHT if chr == MIDDLE_NOAD && am == 203 => r.math_middle(md)?,
        cmd::EXTENSION if chr >= 6 => crate::pdfprim::extension(r, md)?,
        _ => return Ok(false),
    }
    Ok(true)
}

/// `\marks` (pdfTeX §1101).
fn marks(r: &mut Run<'_, '_>, md: &Mode) -> R<()> {
    let c = r.scan_register_num()?;
    let (_, body) = r.scan_toks(false, true)?;
    let v = r.v;
    let toks = toks_value(v, &body);
    let payload = if c == 0 {
        toks
    } else {
        let classes = r
            .get("markclasses")?
            .map(|x| list_of(v, x))
            .unwrap_or_default();
        if !classes.iter().any(|&x| int_of(v, x) == i64::from(c)) {
            let mut cl = classes;
            cl.push(v.int(i64::from(c)));
            r.set("markclasses", v.list(cl), true);
        }
        v.list(vec![v.int(-1), v.int(i64::from(c)), toks])
    };
    let node = v.list(vec![v.int(nodes::T_MARK), payload]);
    r.append(md.lid, node);
    Ok(())
}

fn mark_field(kind: &str, n: i32) -> String {
    if n == 0 {
        format!("mark:{kind}")
    } else {
        format!("marks:{kind}:{n}")
    }
}

/// The loop-back field holding the marks a document node sees
/// (`marks:before:<node>`): the marks in force after the last page the
/// page builder fired before the node's read, as `[[class, top, first,
/// bot], ...]`, or UNIT while no page fired.
pub(crate) fn marks_before_field(node: u64) -> String {
    format!("marks:before:{node}")
}

/// `\topmark` and its kin as read by a run (§386): in a page step (the
/// output routine) and for the split marks, the sequential value; in a
/// document node, its `marks:before:<node>` loop-back, which the
/// `Breaks` fold publishes at the point marked by a `T_MARKS_AT` item
/// this read contributes to the main vertical list.
pub(crate) fn mark_read(r: &mut Run<'_, '_>, kind: &str, n: i32) -> R<Option<ValueId>> {
    let v = r.v;
    let op = r.cx.op();
    let split = kind.starts_with("split");
    // (a text paging inline, `pageinline.rs`, has its marks in force
    // sequentially, as a page step does)
    if split || op == r.ops.pagestep || op == r.ops.pagestep_any || r.pg.is_some() {
        return r.get(&mark_field(kind, n));
    }
    let node = r.cx.node().raw();
    let f = v.field(&marks_before_field(node));
    if !r.cx.is_loopback(f) {
        // a node the client did not make (a document file's region, a
        // display): it asks the client to declare its loop-back
        // (`marksdecl:<node>`, answered by `api::Graph::settle`) and
        // waits; never seen inside a run already declared
        r.cx.use_env(v.field(&format!("marksdecl:{node}")))?;
        if !r.cx.is_loopback(f) {
            return r.get(&mark_field(kind, n));
        }
    }
    let at = v.list(vec![
        v.int(nodes::T_MARKS_AT),
        v.int(i64::from_ne_bytes(node.to_ne_bytes())),
    ]);
    let item = v.list(vec![v.int(0), at]);
    let items = v.field("items");
    let seen = r
        .contribs
        .iter()
        .any(|&(g, s)| g == items && v.seq_to_vec(s).contains(&item));
    if !seen {
        r.contribute("items", v.seq_unit(item));
    }
    let Some(state) = r.cx.use_field(f)?.filter(|&x| x != ValueId::UNIT) else {
        return Ok(None);
    };
    let k = match kind {
        "top" => 1,
        "first" => 2,
        _ => 3,
    };
    Ok(list_of(v, state)
        .into_iter()
        .map(|e| list_of(v, e))
        .find(|e| e.first().is_some_and(|&c| int_of(v, c) == i64::from(n)))
        .and_then(|e| e.get(k).copied())
        .filter(|&x| x != ValueId::UNIT))
}

/// A mark's class and text (a class mark is `[-1, class, text]`).
pub(crate) fn mark_class(v: &phitex_defuse::Values, m: ValueId) -> (i32, ValueId) {
    let xs = list_of(v, m);
    if xs.len() == 3
        && int_of(v, xs[0]) == -1
        && matches!(&*v.get(xs[0]), phitex_defuse::Value::Int(_))
    {
        (i32::try_from(int_of(v, xs[1])).unwrap_or(0), xs[2])
    } else {
        (0, m)
    }
}

/// Whether a mark belongs to a class other than 0.
pub(crate) fn is_class_mark(v: &phitex_defuse::Values, m: Option<ValueId>) -> bool {
    m.is_some_and(|m| mark_class(v, m).0 != 0)
}

/// `fire_up`'s marks of the classes other than 0 (pdfTeX §1012–§1014), the page's marks given as values.
pub(crate) fn fire_marks_values(r: &mut Run<'_, '_>, page_marks: &[ValueId]) -> R<()> {
    let v = r.v;
    let classes: Vec<i32> = r
        .get("markclasses")?
        .map(|x| list_of(v, x))
        .unwrap_or_default()
        .iter()
        .map(|&x| i32::try_from(int_of(v, x)).unwrap_or(0))
        .collect();
    for c in classes {
        let bot = r
            .get(&mark_field("bot", c))?
            .filter(|&x| x != ValueId::UNIT);
        let mut top = r
            .get(&mark_field("top", c))?
            .filter(|&x| x != ValueId::UNIT);
        let mut first = r
            .get(&mark_field("first", c))?
            .filter(|&x| x != ValueId::UNIT);
        if bot.is_some() {
            top = bot;
            first = None;
        }
        let mut new_bot = bot;
        for &m in page_marks {
            let (mc, text) = mark_class(v, m);
            if mc != c {
                continue;
            }
            if first.is_none() {
                first = Some(text);
            }
            new_bot = Some(text);
        }
        if top.is_some() && first.is_none() {
            first = top;
        }
        for (kind, m) in [("top", top), ("first", first), ("bot", new_bot)] {
            r.set(&mark_field(kind, c), m.unwrap_or(ValueId::UNIT), true);
        }
    }
    Ok(())
}

/// `\vsplit`'s marks (§977, §979, pdfTeX `do_marks(vsplit_init)`): every
/// class's split marks are cleared, then set from the part split off.
pub(crate) fn split_marks(r: &mut Run<'_, '_>, split: &[u32], side: &nodes::Side) -> R<()> {
    let v = r.v;
    let mut classes: Vec<i32> = r
        .get("markclasses")?
        .map(|x| list_of(v, x))
        .unwrap_or_default()
        .iter()
        .map(|&x| i32::try_from(int_of(v, x)).unwrap_or(0))
        .collect();
    classes.insert(0, 0);
    for c in classes {
        let mut first = None;
        let mut bot = None;
        for &k in split {
            let Some(&m) = side.marks.get(k as usize) else {
                continue;
            };
            let (mc, text) = mark_class(v, m);
            if mc != c {
                continue;
            }
            if first.is_none() {
                first = Some(text);
            }
            bot = Some(text);
        }
        for (kind, m) in [("splitfirst", first), ("splitbot", bot)] {
            let f = mark_field(kind, c);
            let old = r.get(&f)?.filter(|&x| x != ValueId::UNIT);
            if old != m {
                r.set(&f, m.unwrap_or(ValueId::UNIT), true);
            }
        }
    }
    Ok(())
}

// --------------------------------------------------------- `\middle`

impl Run<'_, '_> {
    /// `\middle` (pdfTeX §1191): the math-left group ends at a right
    /// delimiter marked as middle, and a new one begins with what it
    /// held.
    fn math_middle(&mut self, md: &Mode) -> R<()> {
        let v = self.v;
        if md.group != crate::val::group::MATH_LEFT {
            let _ = self.scan_delimiter(false)?;
            return self.log("error", "Extra \\middle");
        }
        let d = self.scan_delimiter(false)?;
        let mid = v.list(vec![v.int(crate::math::T_RIGHT), v.int(d), v.int(1)]);
        let list = self.fin_mlist(md.lid, Some(mid))?;
        // the \left group closes and a new one opens (pdfTeX §1191)
        self.pop_level();
        let sid = self.push_list(-203, md.group, md.extra.clone());
        for x in list {
            self.append(sid, x);
        }
        Ok(())
    }
}

// ------------------------------------------------- `\show...` (e-TeX)

/// `\showgroups`, `\showtokens`, `\showifs` (pdfTeX §1293, §1702–§1708):
/// shown in the log.
pub(crate) fn show(r: &mut Run<'_, '_>) -> R<()> {
    let esc = i64::from(r.ipar(Par::escapechar)?);
    let mut s = String::new();
    match r.cur.chr {
        5 => {
            // \showtokens: the token list, `> ...`
            let (_, body) = r.scan_toks(false, false)?;
            r.show_toks_into(&mut s, &body)?;
            s.push('.');
            return r.log("show", &s);
        }
        4 => {
            // \showgroups
            let md = r.mode()?;
            let mut lines = Vec::new();
            // the run's own groups first, innermost out (pdfTeX §1293)
            let mut d = md.depth;
            for l in r.nest.iter().rev().filter(|l| l.group != 0) {
                lines.push(format!("### {} group (level {d})", group_name(l.group)));
                d -= 1;
            }
            lines.push("### bottom level".to_owned());
            s = lines.join("\n");
        }
        _ => {
            // \showifs
            let st = if_stack(r);
            if st.is_empty() {
                s.push_str("### no active conditionals");
            } else {
                let n = st.len();
                let mut lines = Vec::new();
                for (i, e) in st.iter().enumerate().rev() {
                    let mut line = format!("### level {}: ", i + 1);
                    let code = e.code.max(0) % UNLESS_CODE;
                    if e.code >= UNLESS_CODE {
                        crate::print::print_esc(&mut line, "unless", esc);
                    }
                    let name = crate::val::prim_name(cmd::IF_TEST, code).unwrap_or("if");
                    crate::print::print_esc(&mut line, name, esc);
                    if e.limit == 2 {
                        crate::print::print_esc(&mut line, "else", esc);
                    }
                    lines.push(line);
                }
                let _ = n;
                s = lines.join("\n");
            }
        }
    }
    r.log("diag", &s)
}

fn group_name(g: i64) -> &'static str {
    match g {
        1 => "simple",
        2 | 3 => "hbox",
        4 => "vbox",
        5 => "vtop",
        6 => "align",
        7 => "no align",
        8 => "output",
        9 => "math",
        10 => "disc",
        11 => "insert",
        12 => "vcenter",
        13 => "math choice",
        14 => "semi simple",
        15 => "math shift",
        16 => "math left",
        _ => "bottom level",
    }
}

/// Whether `t` names a primitive whose meaning it still has.
pub(crate) fn _unused(_: Halt) {}
