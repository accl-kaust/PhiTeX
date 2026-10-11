//! C5: math mode: `$` `$$`, noads, `\over` & co, `\left` `\middle` `\right`,
//! `\mathchoice`, `\eqno`, display placement, and `mlist_to_hlist` (pure).
//!
//! The math list is emitted like any list. Things TeX does by changing the
//! tail (scripts, `\limits`, a group's field, `\over`) are emitted as marker
//! noads and folded when the list closes (`fold`, TeX's `fin_mlist`).
use std::sync::Arc;

use crate::arith::{half, nx_plus_y, x_over_n, xn_over_d};
use crate::ctx::Ctx;
use crate::env::{GroupKind, Mode};
use crate::font::{Font, Fonts, LigKern};
use crate::item::{BoxNode, Item, RUNNING};
use crate::pack::{Params, Spec};
use crate::prim::{dp, gp, ip, tag, tp, Kind, Level, Registry};
use crate::term::{Emit, TokKind};
use crate::tok::Tok;
use crate::val::{FontId, Glue, Key, Meaning, Val};

pub const MAX_DIMEN: i32 = 0o7777777777;
pub const DEFAULT_CODE: i32 = 0o10000000000;
/// `\nullfont`'s id.
pub const NULL_FONT: FontId = 0;

pub const DISPLAY: u8 = 0;
pub const TEXT: u8 = 2;
pub const SCRIPT: u8 = 4;
pub const SCRIPTSCRIPT: u8 = 6;

// TeX's noad types.
pub const ORD: u8 = 16;
pub const OP: u8 = 17;
pub const BIN: u8 = 18;
pub const REL: u8 = 19;
pub const OPEN: u8 = 20;
pub const CLOSE: u8 = 21;
pub const PUNCT: u8 = 22;
pub const INNER: u8 = 23;
pub const RADICAL: u8 = 24;
pub const FRACTION: u8 = 25;
pub const UNDER: u8 = 26;
pub const OVER: u8 = 27;
pub const ACCENT: u8 = 28;
pub const VCENTER: u8 = 29;
pub const LEFT: u8 = 30;
pub const RIGHT: u8 = 31;
const MIDDLE: u16 = 32;

const K_EQNO: u32 = 1;
const K_CHOICE: u32 = 2;
const K_VSPEC: u32 = 3;

/// A noad field (`math_type`).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Field {
    #[default]
    Empty,
    Char { fam: u8, c: u16 },
    TextChar { fam: u8, c: u16 },
    Box(Arc<BoxNode>),
    List(Arc<[Item]>),
    /// To be filled by the math group that follows (builder only).
    Pending,
}

fn empty(f: &Field) -> bool {
    matches!(f, Field::Empty | Field::Pending)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Delim {
    pub small_fam: u8,
    pub small_char: u8,
    pub large_fam: u8,
    pub large_char: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Noad {
    /// ord..inner, radical (`delim`), under, over, accent (`accent`), vcenter.
    /// `subtype`: op 0 displaylimits 1 limits 2 nolimits.
    Simple { ty: u8, subtype: u8, nucleus: Field, sup: Field, sub: Field, delim: Delim, accent: (u8, u16) },
    /// `thickness` `DEFAULT_CODE`: the default rule.
    Fraction { thickness: i32, num: Arc<[Item]>, den: Arc<[Item]>, left: Delim, right: Delim },
    /// `\left` (ty 30) / `\right` (31) / `\middle` (31, middle).
    Fence { ty: u8, middle: bool, delim: Delim },
    Style(u8),
    Choice([Arc<[Item]>; 4]),
    // ---- builder markers, folded away when the list closes
    Script { sup: bool, field: Field },
    Fill(Arc<[Item]>),
    Limits(u8),
    Over { thickness: i32, left: Delim, right: Delim },
    ChoiceArm(u8, Arc<[Item]>),
}

fn simple(ty: u8, nucleus: Field) -> Noad {
    Noad::Simple { ty, subtype: 0, nucleus, sup: Field::Empty, sub: Field::Empty, delim: Delim::default(), accent: (0, 0) }
}

fn noad(c: &mut dyn Ctx, n: Noad) {
    c.item(Item::Noad(Arc::new(n)));
}

// ------------------------------------------------------------------ registry

pub fn register(r: &mut Registry) {
    r.add("mathchar", 0, Kind::Command(cmd_mathchar), 0);
    r.add("delimiter", 0, Kind::Command(cmd_delimiter), 0);
    for (n, t) in [
        ("mathord", ORD), ("mathop", OP), ("mathbin", BIN), ("mathrel", REL), ("mathopen", OPEN),
        ("mathclose", CLOSE), ("mathpunct", PUNCT), ("mathinner", INNER), ("underline", UNDER),
        ("overline", OVER),
    ] {
        r.add(n, t as u16, Kind::Command(cmd_math_comp), 0);
    }
    r.add("displaylimits", 0, Kind::Command(cmd_limits), 0);
    r.add("limits", 1, Kind::Command(cmd_limits), 0);
    r.add("nolimits", 2, Kind::Command(cmd_limits), 0);
    r.add("mathaccent", 0, Kind::Command(cmd_mathaccent), 0);
    r.add("radical", 0, Kind::Command(cmd_radical), 0);
    for (i, n) in ["above", "over", "atop", "abovewithdelims", "overwithdelims", "atopwithdelims"].iter().enumerate() {
        r.add(n, i as u16, Kind::Command(cmd_fraction), 0);
    }
    r.add("left", LEFT as u16, Kind::Command(cmd_left_right), 0);
    r.add("right", RIGHT as u16, Kind::Command(cmd_left_right), 0);
    r.add("middle", MIDDLE, Kind::Command(cmd_left_right), 1);
    r.add("mathchoice", 0, Kind::Command(cmd_mathchoice), 0);
    r.add("displaystyle", DISPLAY as u16, Kind::Command(cmd_style), 0);
    r.add("textstyle", TEXT as u16, Kind::Command(cmd_style), 0);
    r.add("scriptstyle", SCRIPT as u16, Kind::Command(cmd_style), 0);
    r.add("scriptscriptstyle", SCRIPTSCRIPT as u16, Kind::Command(cmd_style), 0);
    r.add("nonscript", 0, Kind::Command(cmd_nonscript), 0);
    r.add("mskip", 0, Kind::Command(cmd_mskip), 0);
    r.add("mkern", 0, Kind::Command(cmd_mkern), 0);
    r.add("vcenter", 0, Kind::Command(cmd_vcenter), 0);
    r.add("eqno", 0, Kind::Command(cmd_eqno), 0);
    r.add("leqno", 1, Kind::Command(cmd_eqno), 0);
    r.add("textfont", 0, Kind::Quantity { level: Level::Ident, key: fam_key }, 0);
    r.add("scriptfont", 1, Kind::Quantity { level: Level::Ident, key: fam_key }, 0);
    r.add("scriptscriptfont", 2, Kind::Quantity { level: Level::Ident, key: fam_key }, 0);
}

fn fam_key(c: &mut dyn Ctx, code: u16) -> Key {
    let f = crate::scan::scan_four_bit_int(c);
    Key::FamFont(code as u8, f)
}

// ------------------------------------------------------------------ helpers

fn self_tok(c: &mut dyn Ctx, name: &str) -> Tok {
    Tok::Cs(c.names().intern(name))
}

fn in_math(c: &mut dyn Ctx) -> bool {
    matches!(c.mode(), Mode::Math { .. })
}

/// `insert_dollar_sign` (tex.web 1047) for the command token `t`.
pub fn insert_dollar_sign(c: &mut dyn Ctx, t: Tok) {
    c.back(t);
    c.push(Arc::from(vec![Tok::Char { cat: 3, ch: '$' }]), TokKind::Inserted);
    crate::diag::error(
        c,
        "Missing $ inserted",
        &["I've inserted a begin-math/end-math symbol since I think", "you left one out. Proceed, with fingers crossed."],
    );
}

/// True in math mode; otherwise inserts `$` before the command named `name`.
fn need_math(c: &mut dyn Ctx, name: &str) -> bool {
    if in_math(c) {
        return true;
    }
    let t = self_tok(c, name);
    insert_dollar_sign(c, t);
    false
}


/// `off_save` (tex.web 1064) for the token `t`.
fn off_save(c: &mut dyn Ctx, t: Tok, what: &str) {
    let g = c.group_kind();
    if g == GroupKind::Bottom {
        crate::diag::error(c, &format!("Extra {what}"), &["Things are pretty mixed up, but I think the worst is over."]);
        return;
    }
    c.back(t);
    let (ins, shown): (Vec<Tok>, &str) = match g {
        GroupKind::SemiSimple => (vec![self_tok(c, "endgroup")], "\\endgroup"),
        GroupKind::MathShift => (vec![Tok::Char { cat: 3, ch: '$' }], "$"),
        GroupKind::MathLeft => (vec![self_tok(c, "right"), Tok::Char { cat: 12, ch: '.' }], "\\right."),
        _ => (vec![Tok::Char { cat: 2, ch: '}' }], "}"),
    };
    c.push(Arc::from(ins), TokKind::Inserted);
    crate::diag::error(
        c,
        &format!("Missing {shown} inserted"),
        &[
            "I've inserted something that you may have forgotten. (See the",
            "<inserted text> above.) With luck, this will get me unwedged. But if you",
            "really didn't forget anything, try typing `2' now; then",
            "my insertion and my current dilemma will both disappear.",
        ],
    );
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cl {
    Letter(char),
    CharGiven(u32),
    MathGiven(u32),
    CharNum,
    MathCharNum,
    DelimNum,
    Relax,
    Space,
    MathShift,
    Other,
}

fn cl_cat(cat: u8, ch: char) -> Cl {
    match cat {
        3 => Cl::MathShift,
        10 => Cl::Space,
        11 | 12 => Cl::Letter(ch),
        _ => Cl::Other,
    }
}

fn classify(c: &mut dyn Ctx, t: Tok) -> Cl {
    match t {
        Tok::Char { cat, ch } => cl_cat(cat, ch),
        Tok::Cs(_) | Tok::Active(_) => match c.meaning(&t) {
            Meaning::Char { cat, ch } => cl_cat(cat, ch),
            Meaning::CharGiven(n) => Cl::CharGiven(n),
            Meaning::MathCharGiven(n) => Cl::MathGiven(n),
            Meaning::Prim(id) => match crate::prim::registry().get(id).name {
                "char" => Cl::CharNum,
                "mathchar" => Cl::MathCharNum,
                "delimiter" => Cl::DelimNum,
                "relax" => Cl::Relax,
                _ => Cl::Other,
            },
            _ => Cl::Other,
        },
        _ => Cl::Other,
    }
}

/// Get the next non-blank non-relax non-call token.
fn next_nbnr(c: &mut dyn Ctx) -> Option<Tok> {
    loop {
        let t = c.next_x()?;
        match classify(c, t) {
            Cl::Space | Cl::Relax => {}
            _ => return Some(t),
        }
    }
}

fn cur_fam(c: &mut dyn Ctx) -> i32 {
    c.int(&Key::IntPar(ip::of("fam")))
}

fn math_field(c: &mut dyn Ctx, code: u32) -> Field {
    let f = cur_fam(c);
    let fam = if code >= 0x7000 && (0..16).contains(&f) { f as u8 } else { ((code / 256) % 16) as u8 };
    Field::Char { fam, c: (code % 256) as u16 }
}

fn mathcode(c: &mut dyn Ctx, ch: u32) -> u32 {
    match char::from_u32(ch) {
        Some(x) => c.int(&Key::MathCode(x)) as u32,
        None => ch,
    }
}

enum Scanned {
    F(Field),
    Brace,
}

/// `scan_math` up to the point where a group would open (the caller emits the
/// noad with a `Pending` field first, then calls `open_math_group`).
fn scan_math(c: &mut dyn Ctx) -> Scanned {
    loop {
        let Some(t) = next_nbnr(c) else { return Scanned::F(Field::Empty) };
        let code = match classify(c, t) {
            Cl::Letter(ch) => {
                let m = mathcode(c, ch as u32);
                if m == 0x8000 {
                    c.back(Tok::Active(ch));
                    continue;
                }
                m
            }
            Cl::CharGiven(n) => {
                let m = mathcode(c, n);
                if m == 0x8000 {
                    if let Some(ch) = char::from_u32(n) {
                        c.back(Tok::Active(ch));
                        continue;
                    }
                }
                m
            }
            Cl::CharNum => {
                let n = crate::scan::scan_char_num(c) as u32;
                let m = mathcode(c, n);
                if m == 0x8000 {
                    if let Some(ch) = char::from_u32(n) {
                        c.back(Tok::Active(ch));
                        continue;
                    }
                }
                m
            }
            Cl::MathCharNum => crate::scan::scan_fifteen_bit_int(c),
            Cl::MathGiven(n) => n,
            Cl::DelimNum => crate::scan::scan_twenty_seven_bit_int(c) / 0o10000,
            _ => {
                c.back(t);
                return Scanned::Brace;
            }
        };
        return Scanned::F(math_field(c, code));
    }
}

fn open_math_group(c: &mut dyn Ctx) {
    crate::scan::scan_left_brace(c);
    c.open_list(Mode::Math { display: false });
    c.open_group(GroupKind::Math);
}

/// Emits `n` with its field filled by `scan_math` (`set` puts the field in).
fn with_field(c: &mut dyn Ctx, n: Noad, set: fn(&mut Noad, Field)) {
    let mut n = n;
    match scan_math(c) {
        Scanned::F(f) => {
            set(&mut n, f);
            noad(c, n);
        }
        Scanned::Brace => {
            set(&mut n, Field::Pending);
            noad(c, n);
            open_math_group(c);
        }
    }
}

fn set_nucleus(n: &mut Noad, f: Field) {
    match n {
        Noad::Simple { nucleus, .. } => *nucleus = f,
        Noad::Script { field, .. } => *field = f,
        _ => {}
    }
}

/// `scan_delimiter` (tex.web 1160).
fn scan_delimiter(c: &mut dyn Ctx, r: bool) -> Delim {
    let mut v: i64;
    let mut bad: Option<Tok> = None;
    if r {
        v = crate::scan::scan_twenty_seven_bit_int(c) as i64;
    } else {
        match next_nbnr(c) {
            Some(t) => match classify(c, t) {
                Cl::Letter(ch) => {
                    v = c.int(&Key::DelCode(ch)) as i64;
                    if v < 0 {
                        bad = Some(t);
                    }
                }
                Cl::DelimNum => v = crate::scan::scan_twenty_seven_bit_int(c) as i64,
                _ => {
                    v = -1;
                    bad = Some(t);
                }
            },
            None => v = -1,
        }
    }
    if v < 0 {
        if let Some(t) = bad {
            c.back(t);
        }
        crate::diag::error(
            c,
            "Missing delimiter (. inserted)",
            &[
                "I was expecting to see something like `(' or `\\{' or",
                "`\\}' here. If you typed, e.g., `{' instead of `\\{', you",
                "should probably delete the `{' by typing `1' now, so that",
                "braces don't get unbalanced. Otherwise just proceed.",
                "Acceptable delimiters are characters whose \\delcode is",
                "nonnegative, or you can use `\\delimiter <delimiter code>'.",
            ],
        );
        v = 0;
    }
    Delim {
        small_fam: ((v / 0x100000) % 16) as u8,
        small_char: ((v / 0x1000) % 256) as u8,
        large_fam: ((v / 256) % 16) as u8,
        large_char: (v % 256) as u8,
    }
}

// ------------------------------------------------------------------ commands

/// `set_math_char` (tex.web 1155); `ch` is the character for a `"8000` code.
fn set_math_char(c: &mut dyn Ctx, code: u32, ch: Option<char>) {
    if code >= 0x8000 {
        if let Some(ch) = ch {
            c.back(Tok::Active(ch));
        }
        return;
    }
    let f = math_field(c, code);
    let ty = if code >= 0x7000 { ORD } else { ORD + (code / 0x1000) as u8 };
    noad(c, simple(ty, f));
}

/// A letter, other char, `\chardef`'d char or `\char` in math mode.
pub fn letter(c: &mut dyn Ctx, ch: u32) {
    let m = mathcode(c, ch);
    set_math_char(c, m, char::from_u32(ch));
}

/// A `\mathchardef`'d token in math mode (outside: `$` inserted by the caller's mode check).
pub fn math_given(c: &mut dyn Ctx, v: u32) {
    set_math_char(c, v, None);
}

fn cmd_mathchar(c: &mut dyn Ctx, _: u16) {
    if need_math(c, "mathchar") {
        let v = crate::scan::scan_fifteen_bit_int(c);
        set_math_char(c, v, None);
    }
}

fn cmd_delimiter(c: &mut dyn Ctx, _: u16) {
    if need_math(c, "delimiter") {
        let v = crate::scan::scan_twenty_seven_bit_int(c) / 0o10000;
        set_math_char(c, v, None);
    }
}

fn comp_name(t: u8) -> &'static str {
    match t {
        ORD => "mathord",
        OP => "mathop",
        BIN => "mathbin",
        REL => "mathrel",
        OPEN => "mathopen",
        CLOSE => "mathclose",
        PUNCT => "mathpunct",
        INNER => "mathinner",
        UNDER => "underline",
        _ => "overline",
    }
}

fn cmd_math_comp(c: &mut dyn Ctx, code: u16) {
    if need_math(c, comp_name(code as u8)) {
        with_field(c, simple(code as u8, Field::Empty), set_nucleus);
    }
}

fn cmd_limits(c: &mut dyn Ctx, code: u16) {
    let n = ["displaylimits", "limits", "nolimits"][code as usize % 3];
    if need_math(c, n) {
        noad(c, Noad::Limits(code as u8));
    }
}

fn cmd_radical(c: &mut dyn Ctx, _: u16) {
    if need_math(c, "radical") {
        let d = scan_delimiter(c, true);
        let mut n = simple(RADICAL, Field::Empty);
        if let Noad::Simple { delim, .. } = &mut n {
            *delim = d;
        }
        with_field(c, n, set_nucleus);
    }
}

fn cmd_mathaccent(c: &mut dyn Ctx, _: u16) {
    if need_math(c, "mathaccent") {
        math_ac(c, false);
    }
}

/// `math_ac` (tex.web 1165); `from_accent`: called for `\accent` in math mode.
pub fn math_ac(c: &mut dyn Ctx, from_accent: bool) {
    if from_accent {
        crate::diag::error(
            c,
            "Please use \\mathaccent for accents in math mode",
            &["I'm changing \\accent to \\mathaccent here; wish me luck.", "(Accents are not the same in formulas as they are in text.)"],
        );
    }
    let v = crate::scan::scan_fifteen_bit_int(c);
    let f = cur_fam(c);
    let fam = if v >= 0x7000 && (0..16).contains(&f) { f as u8 } else { ((v / 256) % 16) as u8 };
    let mut n = simple(ACCENT, Field::Empty);
    if let Noad::Simple { accent, .. } = &mut n {
        *accent = (fam, (v % 256) as u16);
    }
    with_field(c, n, set_nucleus);
}

/// `^` (`sup`) or `_` (cat 7/8 token `t`).
pub fn sub_sup(c: &mut dyn Ctx, t: Tok, sup: bool) {
    if !in_math(c) {
        insert_dollar_sign(c, t);
        return;
    }
    with_field(c, Noad::Script { sup, field: Field::Empty }, set_nucleus);
}

/// `{` in math mode (already consumed): a new ord noad whose nucleus is the group.
pub fn left_brace(c: &mut dyn Ctx) {
    noad(c, simple(ORD, Field::Pending));
    c.open_list(Mode::Math { display: false });
    c.open_group(GroupKind::Math);
}

/// A box made in math mode (`box_end`, `\indent`): an ord noad with a sub_box.
pub fn box_noad(c: &mut dyn Ctx, b: Arc<BoxNode>) {
    noad(c, simple(ORD, Field::Box(b)));
}

fn cmd_fraction(c: &mut dyn Ctx, code: u16) {
    let n = ["above", "over", "atop", "abovewithdelims", "overwithdelims", "atopwithdelims"][code as usize % 6];
    if !need_math(c, n) {
        return;
    }
    let (mut left, mut right) = (Delim::default(), Delim::default());
    if code >= 3 {
        left = scan_delimiter(c, false);
        right = scan_delimiter(c, false);
    }
    let thickness = match code % 3 {
        0 => crate::scan::scan_normal_dimen(c),
        1 => DEFAULT_CODE,
        _ => 0,
    };
    noad(c, Noad::Over { thickness, left, right });
}

fn cmd_left_right(c: &mut dyn Ctx, code: u16) {
    let name = match code {
        30 => "left",
        31 => "right",
        _ => "middle",
    };
    if !need_math(c, name) {
        return;
    }
    let g = c.group_kind();
    if code != LEFT as u16 && g != GroupKind::MathLeft {
        if g == GroupKind::MathShift {
            scan_delimiter(c, false);
            let h = if code == MIDDLE {
                "I'm ignoring a \\middle that had no matching \\left."
            } else {
                "I'm ignoring a \\right that had no matching \\left."
            };
            crate::diag::error(c, &format!("Extra \\{name}"), &[h]);
        } else {
            let t = self_tok(c, name);
            off_save(c, t, name);
        }
        return;
    }
    let delim = scan_delimiter(c, false);
    let p = Noad::Fence { ty: if code == LEFT as u16 { LEFT } else { RIGHT }, middle: code == MIDDLE, delim };
    if code == LEFT as u16 {
        c.open_list(Mode::Math { display: false });
        c.open_group(GroupKind::MathLeft);
        noad(c, p);
        return;
    }
    let raw = c.close_list();
    let q = fold(c, raw, Some(p));
    c.close_group();
    if code == MIDDLE {
        c.open_list(Mode::Math { display: false });
        c.open_group(GroupKind::MathLeft);
        for it in q {
            c.item(it);
        }
    } else {
        noad(c, simple(INNER, Field::List(q.into())));
    }
}

fn cmd_mathchoice(c: &mut dyn Ctx, _: u16) {
    if need_math(c, "mathchoice") {
        open_choice(c, 0);
    }
}

fn open_choice(c: &mut dyn Ctx, k: i32) {
    c.open_list(Mode::Math { display: false });
    c.open_group(GroupKind::MathChoice);
    c.define(Key::Other(tag::MATH, K_CHOICE), Val::Int(k), false);
    crate::scan::scan_left_brace(c);
}

fn cmd_style(c: &mut dyn Ctx, code: u16) {
    let n = match code as u8 {
        DISPLAY => "displaystyle",
        TEXT => "textstyle",
        SCRIPT => "scriptstyle",
        _ => "scriptscriptstyle",
    };
    if need_math(c, n) {
        noad(c, Noad::Style(code as u8));
    }
}

fn cmd_nonscript(c: &mut dyn Ctx, _: u16) {
    if need_math(c, "nonscript") {
        c.item(Item::Glue { g: Glue::default(), sub: 98, leader: None });
    }
}

fn cmd_mskip(c: &mut dyn Ctx, _: u16) {
    if need_math(c, "mskip") {
        let g = crate::scan::scan_glue(c, Level::Mu);
        c.item(Item::Glue { g, sub: 99, leader: None });
    }
}

fn cmd_mkern(c: &mut dyn Ctx, _: u16) {
    if need_math(c, "mkern") {
        let (w, _) = crate::scan::scan_dimen(c, true, false);
        c.item(Item::Kern { w, sub: 3 });
    }
}

/// `normal_paragraph` (tex.web 1070), local to the current group.
fn normal_paragraph(c: &mut dyn Ctx) {
    let lo = Key::IntPar(ip::of("looseness"));
    if c.int(&lo) != 0 {
        c.define(lo, Val::Int(0), false);
    }
    let hi = Key::DimenPar(dp::of("hangindent"));
    if c.int(&hi) != 0 {
        c.define(hi, Val::Int(0), false);
    }
    let ha = Key::IntPar(ip::of("hangafter"));
    if c.int(&ha) != 1 {
        c.define(ha, Val::Int(1), false);
    }
    if let Val::Parshape(p) = c.get(&Key::Parshape) {
        if !p.is_empty() {
            c.define(Key::Parshape, Val::Parshape(Arc::from(Vec::new())), false);
        }
    }
    if c.etex() {
        if let Val::Ints(p) = c.get(&Key::PenShape(0)) {
            if !p.is_empty() {
                c.define(Key::PenShape(0), Val::Ints(Arc::from(Vec::new())), false);
            }
        }
    }
}

fn cmd_vcenter(c: &mut dyn Ctx, _: u16) {
    if !need_math(c, "vcenter") {
        return;
    }
    let spec = if crate::scan::scan_keyword(c, "to") {
        [0, crate::scan::scan_normal_dimen(c)]
    } else if crate::scan::scan_keyword(c, "spread") {
        [1, crate::scan::scan_normal_dimen(c)]
    } else {
        [1, 0]
    };
    c.open_group(GroupKind::Vcenter);
    c.define(Key::Other(tag::MATH, K_VSPEC), Val::Ints(Arc::from(spec.to_vec())), false);
    crate::scan::scan_left_brace(c);
    normal_paragraph(c);
    c.open_list(Mode::Vertical { inner: true });
    c.define(Key::PrevDepth, Val::Int(-65_536_000), false);
    let t = c.get(&Key::TokPar(tp::of("everyvbox"))).toks();
    if !t.is_empty() {
        c.push(t, TokKind::EveryVbox);
    }
}

fn cmd_eqno(c: &mut dyn Ctx, code: u16) {
    let name = if code == 1 { "leqno" } else { "eqno" };
    match c.mode() {
        Mode::Math { display: true } => {
            if c.group_kind() == GroupKind::MathShift {
                go_ordinary(c, Some(code as i32));
            } else {
                let t = self_tok(c, name);
                off_save(c, t, name);
            }
        }
        Mode::Math { display: false } => crate::diag::error(
            c,
            &format!("You can't use `\\{name}' in math mode"),
            &[
                "Sorry, but I'm not programmed to handle this case;",
                "I'll just pretend that you didn't ask for it.",
                "If you're in the wrong mode, you might be able to",
                "return to the right one by typing `I}' or `I$' or `I\\par'.",
            ],
        ),
        _ => {
            let t = self_tok(c, name);
            insert_dollar_sign(c, t);
        }
    }
}

// ------------------------------------------------------------------ fold (fin_mlist)

fn last_noad(cur: &[Item]) -> Option<Noad> {
    match cur.last() {
        Some(Item::Noad(n)) => Some((**n).clone()),
        _ => None,
    }
}

fn replace_last(cur: &mut Vec<Item>, n: Noad) {
    cur.pop();
    cur.push(Item::Noad(Arc::new(n)));
}

fn attach_script(c: &mut dyn Ctx, cur: &mut Vec<Item>, sup: bool, field: Field) {
    let mut err = false;
    if let Some(mut n) = last_noad(cur) {
        match &mut n {
            Noad::Simple { sup: s, sub: b, .. } => {
                let slot = if sup { s } else { b };
                if empty(slot) {
                    *slot = field;
                    replace_last(cur, n);
                    return;
                }
                err = true;
            }
            Noad::Fraction { .. } => err = true,
            _ => {}
        }
    }
    if err {
        if sup {
            crate::diag::error(c, "Double superscript", &["I treat `x^1^2' essentially like `x^1{}^2'."]);
        } else {
            crate::diag::error(c, "Double subscript", &["I treat `x_1_2' essentially like `x_1{}_2'."]);
        }
    }
    let mut n = simple(ORD, Field::Empty);
    if let Noad::Simple { sup: s, sub: b, .. } = &mut n {
        if sup {
            *s = field;
        } else {
            *b = field;
        }
    }
    cur.push(Item::Noad(Arc::new(n)));
}

fn fill(cur: &mut Vec<Item>, list: Arc<[Item]>) {
    let Some(mut n) = last_noad(cur) else { return };
    let Noad::Simple { nucleus, sup, sub, .. } = &mut n else { return };
    let slot = if *nucleus == Field::Pending {
        nucleus
    } else if *sup == Field::Pending {
        sup
    } else if *sub == Field::Pending {
        sub
    } else {
        return;
    };
    let mut val = Field::List(list.clone());
    if list.len() == 1 {
        if let Item::Noad(p) = &list[0] {
            match &**p {
                Noad::Simple { ty: ORD, sup: Field::Empty, sub: Field::Empty, nucleus: pn, .. } => val = pn.clone(),
                _ => {}
            }
        }
    }
    *slot = val;
    replace_last(cur, n);
}

/// TeX's `fin_mlist`: folds the markers of a closed math list; `right`: the
/// `\right`/`\middle` that closes a `\left` group.
pub fn fold(c: &mut dyn Ctx, raw: Vec<Item>, right: Option<Noad>) -> Vec<Item> {
    let mut cur: Vec<Item> = Vec::new();
    let mut inc: Option<(i32, Delim, Delim, Vec<Item>)> = None;
    for it in raw {
        let Item::Noad(n) = &it else {
            cur.push(it);
            continue;
        };
        match &**n {
            Noad::Script { sup, field } => attach_script(c, &mut cur, *sup, field.clone()),
            Noad::Fill(list) => {
                // a group that is a lone accent replaces an ord tail whose nucleus it fills
                let acc = list.len() == 1 && matches!(&list[0], Item::Noad(p) if matches!(**p, Noad::Simple { ty: ACCENT, .. }));
                let ord_nuc = matches!(last_noad(&cur), Some(Noad::Simple { ty: ORD, nucleus: Field::Pending, .. }));
                if acc && ord_nuc {
                    cur.pop();
                    cur.push(list[0].clone());
                } else {
                    fill(&mut cur, list.clone());
                }
            }
            Noad::Limits(k) => match last_noad(&cur) {
                Some(Noad::Simple { ty: OP, subtype: _, nucleus, sup, sub, delim, accent }) => replace_last(
                    &mut cur,
                    Noad::Simple { ty: OP, subtype: *k, nucleus, sup, sub, delim, accent },
                ),
                _ => crate::diag::error(
                    c,
                    "Limit controls must follow a math operator",
                    &["I'm ignoring this misplaced \\limits or \\nolimits command."],
                ),
            },
            Noad::Over { thickness, left, right } => {
                if inc.is_some() {
                    crate::diag::error(
                        c,
                        "Ambiguous; you need another { and }",
                        &[
                            "I'm ignoring this fraction specification, since I don't",
                            "know whether a construction like `x \\over y \\over z'",
                            "means `{x \\over y} \\over z' or `x \\over {y \\over z}'.",
                        ],
                    );
                } else {
                    inc = Some((*thickness, *left, *right, std::mem::take(&mut cur)));
                }
            }
            Noad::ChoiceArm(k, l) => {
                let k = (*k as usize).min(3);
                match last_noad(&cur) {
                    Some(Noad::Choice(mut arms)) if k > 0 => {
                        arms[k] = l.clone();
                        replace_last(&mut cur, Noad::Choice(arms));
                    }
                    _ => {
                        let e: Arc<[Item]> = Arc::from(Vec::new());
                        let mut arms = [e.clone(), e.clone(), e.clone(), e];
                        arms[k] = l.clone();
                        cur.push(Item::Noad(Arc::new(Noad::Choice(arms))));
                    }
                }
            }
            _ => cur.push(it.clone()),
        }
    }
    let r = right.map(|r| Item::Noad(Arc::new(r)));
    match inc {
        None => {
            cur.extend(r);
            cur
        }
        Some((thickness, left, rd, num)) => {
            let frac = |num: Vec<Item>, den: Vec<Item>| {
                Item::Noad(Arc::new(Noad::Fraction { thickness, num: num.into(), den: den.into(), left, right: rd }))
            };
            match r {
                None => vec![frac(num, cur)],
                Some(r) => {
                    let k = num.iter().rposition(|it| matches!(it, Item::Noad(n) if matches!(**n, Noad::Fence { .. })));
                    let (mut out, numer) = match k {
                        Some(k) => (num[..=k].to_vec(), num[k + 1..].to_vec()),
                        None => (Vec::new(), num),
                    };
                    out.push(frac(numer, cur));
                    out.push(r);
                    out
                }
            }
        }
    }
}

// ------------------------------------------------------------------ group ends

pub fn math_group_end(c: &mut dyn Ctx, kind: GroupKind) {
    match kind {
        GroupKind::Math => {
            let raw = c.close_list();
            let list = fold(c, raw, None);
            c.close_group();
            noad(c, Noad::Fill(list.into()));
        }
        GroupKind::MathChoice => {
            let k = c.int(&Key::Other(tag::MATH, K_CHOICE));
            let raw = c.close_list();
            let list = fold(c, raw, None);
            c.close_group();
            noad(c, Noad::ChoiceArm(k as u8, list.into()));
            if k < 3 {
                open_choice(c, k + 1);
            }
        }
        GroupKind::Vcenter => {
            if c.mode() == (Mode::Horizontal { inner: false }) {
                crate::list::end_graf(c);
            }
            let spec = match c.get(&Key::Other(tag::MATH, K_VSPEC)) {
                Val::Ints(v) if v.len() == 2 && v[0] == 0 => Spec::Exactly(v[1]),
                Val::Ints(v) if v.len() == 2 => Spec::Additional(v[1]),
                _ => Spec::Additional(0),
            };
            let items = c.close_list();
            c.close_group();
            let pp = pack_params(c);
            let pk = crate::pack::vpack(items, spec, MAX_DIMEN, &pp);
            if let Some(d) = pk.report {
                c.emit(Emit::Diag(d));
            }
            noad(c, simple(VCENTER, Field::Box(Arc::new(pk.b))));
        }
        GroupKind::MathShift | GroupKind::MathLeft => {
            let w = if kind == GroupKind::MathShift { "$" } else { "\\right" };
            crate::diag::error(
                c,
                &format!("Extra }}, or forgotten {w}"),
                &[
                    "I've deleted a group-closing symbol because it seems to be",
                    "spurious, as in `$x}$'. But perhaps the } is legitimate and",
                    "you forgot something else, as in `\\hbox{$x}'. In such cases",
                    "the way to recover is to insert both the forgotten and the",
                    "deleted material, e.g., by typing `I$}'.",
                ],
            );
        }
        _ => {}
    }
}

fn pack_params(c: &mut dyn Ctx) -> Params {
    Params {
        hbadness: c.int(&Key::IntPar(ip::of("hbadness"))),
        vbadness: c.int(&Key::IntPar(ip::of("vbadness"))),
        hfuzz: c.int(&Key::DimenPar(dp::of("hfuzz"))),
        vfuzz: c.int(&Key::DimenPar(dp::of("vfuzz"))),
        overfull_rule: c.int(&Key::DimenPar(dp::of("overfullrule"))),
    }
}

// ------------------------------------------------------------------ $ and $$

fn is_math_shift(c: &mut dyn Ctx, t: Tok) -> bool {
    classify(c, t) == Cl::MathShift
}

/// `$` in any mode (the token itself is `t`).
pub fn math_shift(c: &mut dyn Ctx, t: Tok) {
    match c.mode() {
        Mode::Vertical { .. } => {
            c.back(t);
            crate::list::new_graf(c, true);
        }
        Mode::Horizontal { .. } => init_math_h(c),
        Mode::Math { .. } => {
            if c.group_kind() == GroupKind::MathShift {
                after_math(c);
            } else {
                off_save(c, t, "$");
            }
        }
    }
}

/// `$` in horizontal mode or a math group's end.
pub fn init_math(c: &mut dyn Ctx) {
    math_shift(c, Tok::Char { cat: 3, ch: '$' });
}

fn init_math_h(c: &mut dyn Ctx) {
    let t = c.next();
    let shift = match t {
        Some(t) => is_math_shift(c, t),
        None => false,
    };
    if shift && c.mode() == (Mode::Horizontal { inner: false }) {
        go_display(c);
    } else {
        if let Some(t) = t {
            c.back(t);
        }
        go_ordinary(c, None);
    }
}

fn go_ordinary(c: &mut dyn Ctx, eqno: Option<i32>) {
    c.open_list(Mode::Math { display: false });
    c.open_group(GroupKind::MathShift);
    c.define(Key::IntPar(ip::of("fam")), Val::Int(-1), false);
    if let Some(k) = eqno {
        c.define(Key::Other(tag::MATH, K_EQNO), Val::Int(k), false);
    }
    let t = c.get(&Key::TokPar(tp::of("everymath"))).toks();
    if !t.is_empty() {
        c.push(t, TokKind::EveryMath);
    }
}

/// tex.web 1146–1148: the natural width of the paragraph's last line.
fn pre_display_size(c: &mut dyn Ctx, b: &BoxNode) -> i32 {
    let fonts = crate::font::fonts();
    let quad = match c.get(&Key::CurFont) {
        Val::Font(id) => fonts.font(id).param(6),
        _ => 0,
    };
    let mut v = b.shift + 2 * quad;
    let mut w = -MAX_DIMEN;
    for it in b.list.iter() {
        let (d, vis) = match it {
            Item::Char { font, c } | Item::Lig { font, c, .. } => (fonts.font(*font).width(*c), true),
            Item::Box(x) => (x.width, true),
            Item::Rule { w, .. } => (*w, true),
            Item::Kern { w, .. } | Item::Math { w, .. } | Item::MarginKern { w, .. } => (*w, false),
            Item::Glue { g, sub, leader } => {
                if b.set.sign == 1 {
                    if b.set.order == g.stretch_order && g.stretch != 0 {
                        v = MAX_DIMEN;
                    }
                } else if b.set.sign == -1 && b.set.order == g.shrink_order && g.shrink != 0 {
                    v = MAX_DIMEN;
                }
                (g.width, leader.is_some() || *sub >= 100)
            }
            _ => (0, false),
        };
        if vis {
            if v < MAX_DIMEN {
                v += d;
                w = v;
            } else {
                w = MAX_DIMEN;
                break;
            }
        } else if v < MAX_DIMEN {
            v += d;
        }
    }
    w
}

fn go_display(c: &mut dyn Ctx) {
    let just = crate::list::display_break(c);
    let w = match just {
        None => -MAX_DIMEN,
        Some(b) => pre_display_size(c, &b),
    };
    let pg = c.int(&Key::PrevGraf);
    let (l, s) = match c.get(&Key::Parshape) {
        Val::Parshape(ps) if !ps.is_empty() => {
            let n = ps.len() as i32;
            let k = if pg + 2 >= n { n } else { pg + 2 };
            let (ind, len) = ps[(k - 1) as usize];
            (len, ind)
        }
        _ => {
            let hi = c.int(&Key::DimenPar(dp::of("hangindent")));
            let ha = c.int(&Key::IntPar(ip::of("hangafter")));
            let hs = c.int(&Key::DimenPar(dp::of("hsize")));
            if hi != 0 && ((ha >= 0 && pg + 2 > ha) || (pg + 1 < -ha)) {
                (hs - hi.abs(), if hi > 0 { hi } else { 0 })
            } else {
                (hs, 0)
            }
        }
    };
    c.open_list(Mode::Math { display: true });
    c.open_group(GroupKind::MathShift);
    c.define(Key::IntPar(ip::of("fam")), Val::Int(-1), false);
    c.define(Key::DimenPar(dp::of("predisplaysize")), Val::Int(w), false);
    if c.etex() {
        c.define(Key::IntPar(ip::of("predisplaydirection")), Val::Int(0), false);
    }
    c.define(Key::DimenPar(dp::of("displaywidth")), Val::Int(l), false);
    c.define(Key::DimenPar(dp::of("displayindent")), Val::Int(s), false);
    let t = c.get(&Key::TokPar(tp::of("everydisplay"))).toks();
    if !t.is_empty() {
        c.push(t, TokKind::EveryDisplay);
    }
}

/// tex.web 1195: false, or an error and true when the math fonts lack parameters.
fn check_fonts(c: &mut dyn Ctx) -> bool {
    let fonts = crate::font::fonts();
    let mut short = |c: &mut dyn Ctx, fam: u8, need: u16| {
        (0..3u8).any(|s| match c.get(&Key::FamFont(s, fam)) {
            Val::Font(id) if id != NULL_FONT => fonts.font(id).param_count() < need,
            _ => true,
        })
    };
    let (msg, n) = if short(c, 2, 22) {
        ("Math formula deleted: Insufficient symbol fonts", 2)
    } else if short(c, 3, 13) {
        ("Math formula deleted: Insufficient extension fonts", 3)
    } else {
        return false;
    };
    let h0 = format!("Sorry, but I can't typeset math unless \\textfont {n}");
    let h1 = format!("and \\scriptfont {n} and \\scriptscriptfont {n} have all");
    let h2 = if n == 2 {
        "the \\fontdimen values needed in math symbol fonts."
    } else {
        "the \\fontdimen values needed in math extension fonts."
    };
    crate::diag::error(c, msg, &[&h0, &h1, h2]);
    true
}

fn check_dollar(c: &mut dyn Ctx) {
    let t = c.next_x();
    let ok = match t {
        Some(t) => is_math_shift(c, t),
        None => false,
    };
    if !ok {
        if let Some(t) = t {
            c.back(t);
        }
        crate::diag::error(
            c,
            "Display math should end with $$",
            &["The `$' that I just saw supposedly matches a previous `$$'.", "So I shall assume that you typed `$$' both times."],
        );
    }
}

fn report(c: &mut dyn Ctx, out: &Mathed) {
    for (m, h) in &out.errors {
        crate::diag::error(c, m, h);
    }
    for l in &out.lost {
        c.emit(Emit::Write { stream: 17, text: l.as_str().into() });
    }
}

const QUIET: Params = Params { hbadness: 10000, vbadness: 10000, hfuzz: MAX_DIMEN, vfuzz: MAX_DIMEN, overfull_rule: 0 };

fn after_math(c: &mut dyn Ctx) {
    let fonts = crate::font::fonts();
    let mut danger = check_fonts(c);
    let mut m = c.mode();
    let mut params = read_params(c);
    let raw = c.close_list();
    let mut p = if danger { Vec::new() } else { fold(c, raw, None) };
    let mut a: Option<BoxNode> = None;
    let mut l = false;
    if m == (Mode::Math { display: false }) && c.mode() == (Mode::Math { display: true }) {
        check_dollar(c);
        let out = mlist_to_hlist(&p, TEXT, false, &params, fonts);
        report(c, &out);
        let mut bx = crate::pack::hpack(out.list, Spec::Additional(0), &QUIET, fonts).b;
        if c.etex() {
            bx.lr = 2;
        }
        l = c.int(&Key::Other(tag::MATH, K_EQNO)) == 1;
        c.close_group();
        a = Some(bx);
        danger = check_fonts(c);
        m = c.mode();
        params = read_params(c);
        let raw = c.close_list();
        p = if danger { Vec::new() } else { fold(c, raw, None) };
    }
    if m == (Mode::Math { display: false }) {
        // finish math in text (1196)
        let ms = c.int(&Key::DimenPar(dp::of("mathsurround")));
        c.item(Item::Math { w: ms, sub: 0 });
        let pen = c.mode() == (Mode::Horizontal { inner: false });
        let out = mlist_to_hlist(&p, TEXT, pen, &params, fonts);
        report(c, &out);
        for it in out.list {
            c.item(it);
        }
        c.item(Item::Math { w: ms, sub: 1 });
        c.define(Key::SpaceFactor, Val::Int(1000), false);
        c.close_group();
        return;
    }
    if a.is_none() {
        check_dollar(c);
    }
    finish_display(c, p, a, l, danger, &params);
}

fn param_glue(c: &mut dyn Ctx, code: u16) -> Item {
    Item::Glue { g: c.get(&Key::GluePar(code)).glue(), sub: code as u8 + 1, leader: None }
}

/// The box `b` of a natural hpack with vadjust/ins/mark material moved out.
fn migrate(pk: crate::pack::Packed) -> (BoxNode, Vec<Item>) {
    let mut b = pk.b;
    let had = !pk.adjust.is_empty();
    let mut t = pk.adjust;
    let mut kept = Vec::new();
    for it in b.list.iter() {
        match it {
            Item::Ins(_) | Item::Mark { .. } => {
                if !had {
                    t.push(it.clone());
                }
            }
            Item::Adjust(l) => {
                if !had {
                    t.extend(l.iter().cloned());
                }
            }
            _ => kept.push(it.clone()),
        }
    }
    b.list = kept.into();
    (b, t)
}

/// tex.web 1199–1206.
fn finish_display(c: &mut dyn Ctx, p: Vec<Item>, a: Option<BoxNode>, l: bool, danger: bool, params: &MathParams) {
    let fonts = crate::font::fonts();
    let out = mlist_to_hlist(&p, DISPLAY, false, params, fonts);
    report(c, &out);
    let pp = pack_params(c);
    let (mut b, t) = migrate(crate::pack::hpack(out.list, Spec::Additional(0), &pp, fonts));
    let first_is_glue = matches!(b.list.first(), Some(Item::Glue { .. }));
    let mut w = b.width;
    let z = c.int(&Key::DimenPar(dp::of("displaywidth")));
    let mut s = c.int(&Key::DimenPar(dp::of("displayindent")));
    if c.etex() && c.int(&Key::IntPar(ip::of("predisplaydirection"))) < 0 {
        s = -s - z;
    }
    let (mut e, q) = match (&a, danger) {
        (Some(a), false) => {
            let quad = params.fam[0][2].map(|f| fonts.font(f).param(6)).unwrap_or(0);
            (a.width, a.width + quad)
        }
        _ => (0, 0),
    };
    if w + q > z {
        let mut ts = [0i32; 4];
        for it in b.list.iter() {
            if let Item::Glue { g, .. } = it {
                ts[(g.shrink_order as usize).min(3)] += g.shrink;
            }
        }
        let list = b.list.to_vec();
        if e != 0 && (w - ts[0] + q <= z || ts[1] != 0 || ts[2] != 0 || ts[3] != 0) {
            let pk = crate::pack::hpack(list, Spec::Exactly(z - q), &pp, fonts);
            if let Some(d) = pk.report {
                c.emit(Emit::Diag(d));
            }
            b = pk.b;
        } else {
            e = 0;
            if w > z {
                let pk = crate::pack::hpack(list, Spec::Exactly(z), &pp, fonts);
                if let Some(d) = pk.report {
                    c.emit(Emit::Diag(d));
                }
                b = pk.b;
            }
        }
        w = b.width;
    }
    let mut d = half(z - w);
    if e > 0 && d < 2 * e {
        d = half(z - w - e);
        if first_is_glue {
            d = 0;
        }
    }
    let pre = c.int(&Key::IntPar(ip::of("predisplaypenalty")));
    c.item(Item::Penalty(pre));
    let pds = c.int(&Key::DimenPar(dp::of("predisplaysize")));
    let (g1, g2) = if d + s <= pds || l {
        (gp::of("abovedisplayskip"), gp::of("belowdisplayskip"))
    } else {
        (gp::of("abovedisplayshortskip"), gp::of("belowdisplayshortskip"))
    };
    let mut g2 = Some(g2);
    if l && e == 0 {
        if let Some(a) = &a {
            let mut aa = a.clone();
            aa.shift = s;
            crate::list::append_to_vlist(c, Arc::new(aa));
        }
        c.item(Item::Penalty(10000));
    } else {
        let g = param_glue(c, g1);
        c.item(g);
    }
    if e != 0 {
        let a = a.clone().unwrap_or_default();
        let r = Item::Kern { w: z - w - e - d, sub: 0 };
        let list = if l {
            d = 0;
            vec![Item::Box(Arc::new(a)), r, Item::Box(Arc::new(b))]
        } else {
            vec![Item::Box(Arc::new(b)), r, Item::Box(Arc::new(a))]
        };
        b = crate::pack::hpack(list, Spec::Additional(0), &pp, fonts).b;
    }
    b.shift = s + d;
    crate::list::append_to_vlist(c, Arc::new(b));
    if let Some(a) = &a {
        if e == 0 && !l {
            c.item(Item::Penalty(10000));
            let mut aa = a.clone();
            aa.shift = s + z - aa.width;
            crate::list::append_to_vlist(c, Arc::new(aa));
            g2 = None;
        }
    }
    for it in t {
        c.item(it);
    }
    let post = c.int(&Key::IntPar(ip::of("postdisplaypenalty")));
    c.item(Item::Penalty(post));
    if let Some(g2) = g2 {
        let g = param_glue(c, g2);
        c.item(g);
    }
    resume_after_display(c);
}

fn norm_min(h: i32) -> i32 {
    h.clamp(1, 63)
}

fn resume_after_display(c: &mut dyn Ctx) {
    c.close_group();
    let pg = c.int(&Key::PrevGraf);
    c.define(Key::PrevGraf, Val::Int(pg + 3), false);
    c.open_list(Mode::Horizontal { inner: false });
    c.define(Key::SpaceFactor, Val::Int(1000), false);
    let lang = c.int(&Key::IntPar(ip::of("language")));
    let cl = if lang <= 0 || lang > 255 { 0 } else { lang };
    c.define(Key::Clang, Val::Int(cl), false);
    let lh = norm_min(c.int(&Key::IntPar(ip::of("lefthyphenmin"))));
    let rh = norm_min(c.int(&Key::IntPar(ip::of("righthyphenmin"))));
    c.define(Key::PrevGraf, Val::Int((lh * 64 + rh) * 65536 + cl), false);
    if let Some(t) = c.next_x() {
        if classify(c, t) != Cl::Space {
            c.back(t);
        }
    }
}

// ------------------------------------------------------------------ mlist_to_hlist

/// Everything `mlist_to_hlist` reads from env.
#[derive(Clone, Debug, PartialEq)]
pub struct MathParams {
    /// `\textfont` `\scriptfont` `\scriptscriptfont` by (size, family); None: `\nullfont`.
    pub fam: [[Option<FontId>; 16]; 3],
    /// `\skewchar` of each of those fonts.
    pub skew: [[i32; 16]; 3],
    pub thin: Glue,
    pub med: Glue,
    pub thick: Glue,
    pub bin_op_penalty: i32,
    pub rel_penalty: i32,
    pub script_space: i32,
    pub delimiter_factor: i32,
    pub delimiter_shortfall: i32,
    pub null_delimiter_space: i32,
    pub tracing_lost_chars: i32,
}

pub fn read_params(c: &mut dyn Ctx) -> MathParams {
    let mut fam = [[None; 16]; 3];
    let mut skew = [[-1; 16]; 3];
    for s in 0..3u8 {
        for f in 0..16u8 {
            if let Val::Font(id) = c.get(&Key::FamFont(s, f)) {
                if id != NULL_FONT {
                    fam[s as usize][f as usize] = Some(id);
                    skew[s as usize][f as usize] = c.int(&Key::SkewChar(id));
                }
            }
        }
    }
    MathParams {
        fam,
        skew,
        thin: c.get(&Key::MuGluePar(0)).glue(),
        med: c.get(&Key::MuGluePar(1)).glue(),
        thick: c.get(&Key::MuGluePar(2)).glue(),
        bin_op_penalty: c.int(&Key::IntPar(ip::of("binoppenalty"))),
        rel_penalty: c.int(&Key::IntPar(ip::of("relpenalty"))),
        script_space: c.int(&Key::DimenPar(dp::of("scriptspace"))),
        delimiter_factor: c.int(&Key::IntPar(ip::of("delimiterfactor"))),
        delimiter_shortfall: c.int(&Key::DimenPar(dp::of("delimitershortfall"))),
        null_delimiter_space: c.int(&Key::DimenPar(dp::of("nulldelimiterspace"))),
        tracing_lost_chars: c.int(&Key::IntPar(ip::of("tracinglostchars"))),
    }
}

/// The hlist, plus the errors (message, help) and lost-character log lines met.
#[derive(Clone, Debug, Default)]
pub struct Mathed {
    pub list: Vec<Item>,
    pub errors: Vec<(String, Vec<&'static str>)>,
    pub lost: Vec<String>,
}

/// tex.web 719–767: a pure function of the mlist, the style, and the fonts.
pub fn mlist_to_hlist(mlist: &[Item], style: u8, penalties: bool, p: &MathParams, fonts: &dyn Fonts) -> Mathed {
    let mut m = M { p, fonts, errors: Vec::new(), lost: Vec::new() };
    let list = m.mlist(mlist, style, penalties);
    Mathed { list, errors: m.errors, lost: m.lost }
}

fn cramped(s: u8) -> u8 {
    2 * (s / 2) + 1
}
fn sub_style(s: u8) -> u8 {
    2 * (s / 4) + 5
}
fn sup_style(s: u8) -> u8 {
    2 * (s / 4) + 4 + (s % 2)
}
fn num_style(s: u8) -> u8 {
    s + 2 - 2 * (s / 6)
}
fn denom_style(s: u8) -> u8 {
    2 * (s / 2) + 1 + 2 - 2 * (s / 6)
}

fn mu_mult(n: i32, f: i32, x: i32) -> i32 {
    nx_plus_y(n, x, xn_over_d(x, f, 0o200000).0).unwrap_or(0)
}

fn mu_nf(m: i32) -> (i32, i32) {
    let (mut n, mut f) = x_over_n(m, 0o200000);
    if f < 0 {
        n -= 1;
        f += 0o200000;
    }
    (n, f)
}

fn math_glue(g: Glue, m: i32) -> Glue {
    let (n, f) = mu_nf(m);
    Glue {
        width: mu_mult(n, f, g.width),
        stretch: if g.stretch_order == 0 { mu_mult(n, f, g.stretch) } else { g.stretch },
        shrink: if g.shrink_order == 0 { mu_mult(n, f, g.shrink) } else { g.shrink },
        stretch_order: g.stretch_order,
        shrink_order: g.shrink_order,
    }
}

fn print_ascii(c: u16) -> String {
    match c {
        32..=126 => (c as u8 as char).to_string(),
        0..=63 => format!("^^{}", (c as u8 + 64) as char),
        64..=127 => format!("^^{}", (c as u8 - 64) as char),
        _ => format!("^^{:02x}", c),
    }
}

const SPACING: &[u8; 64] = b"0234000122*4000133**3**344*0400400*000000234000111*1111112341011";

#[derive(Clone)]
struct Wn {
    ty: u8,
    subtype: u8,
    nucleus: Field,
    sup: Field,
    sub: Field,
    delim: Delim,
    delim2: Delim,
    accent: (u8, u16),
    thick: i32,
    num: Arc<[Item]>,
    den: Arc<[Item]>,
    hl: Vec<Item>,
}

impl Wn {
    fn new(ty: u8) -> Wn {
        Wn {
            ty,
            subtype: 0,
            nucleus: Field::Empty,
            sup: Field::Empty,
            sub: Field::Empty,
            delim: Delim::default(),
            delim2: Delim::default(),
            accent: (0, 0),
            thick: 0,
            num: Arc::from(Vec::new()),
            den: Arc::from(Vec::new()),
            hl: Vec::new(),
        }
    }
}

enum W {
    N(Box<Wn>),
    Style(u8),
    Choice([Arc<[Item]>; 4]),
    Node(Item),
}

fn to_w(items: &[Item]) -> Vec<W> {
    let mut v = Vec::with_capacity(items.len());
    for it in items {
        let Item::Noad(n) = it else {
            v.push(W::Node(it.clone()));
            continue;
        };
        match &**n {
            Noad::Simple { ty, subtype, nucleus, sup, sub, delim, accent } => {
                let mut w = Wn::new(*ty);
                w.subtype = *subtype;
                w.nucleus = nucleus.clone();
                w.sup = sup.clone();
                w.sub = sub.clone();
                w.delim = *delim;
                w.accent = *accent;
                v.push(W::N(Box::new(w)));
            }
            Noad::Fraction { thickness, num, den, left, right } => {
                let mut w = Wn::new(FRACTION);
                w.thick = *thickness;
                w.num = num.clone();
                w.den = den.clone();
                w.delim = *left;
                w.delim2 = *right;
                v.push(W::N(Box::new(w)));
            }
            Noad::Fence { ty, middle, delim } => {
                let mut w = Wn::new(*ty);
                w.subtype = *middle as u8;
                w.delim = *delim;
                v.push(W::N(Box::new(w)));
            }
            Noad::Style(s) => v.push(W::Style(*s)),
            Noad::Choice(a) => v.push(W::Choice(a.clone())),
            _ => {}
        }
    }
    v
}

fn wn(v: &mut [W], i: usize) -> &mut Wn {
    match &mut v[i] {
        W::N(n) => n,
        _ => unreachable!("not a noad"),
    }
}

fn bx(b: BoxNode) -> Item {
    Item::Box(Arc::new(b))
}

fn kern(w: i32) -> Item {
    Item::Kern { w, sub: 0 }
}

struct M<'a> {
    p: &'a MathParams,
    fonts: &'a dyn Fonts,
    errors: Vec<(String, Vec<&'static str>)>,
    lost: Vec<String>,
}

impl M<'_> {
    fn font(&self, id: FontId) -> Arc<Font> {
        self.fonts.font(id)
    }
    fn fam_font(&self, sz: usize, fam: u8) -> Option<FontId> {
        self.p.fam[sz][(fam & 15) as usize]
    }
    fn sy(&self, k: u16, sz: usize) -> i32 {
        self.fam_font(sz, 2).map(|f| self.font(f).param(k)).unwrap_or(0)
    }
    fn ex(&self, k: u16, sz: usize) -> i32 {
        self.fam_font(sz, 3).map(|f| self.font(f).param(k)).unwrap_or(0)
    }
    fn axis(&self, sz: usize) -> i32 {
        self.sy(22, sz)
    }
    fn size_mu(&self, style: u8) -> (usize, i32) {
        let sz = if style < SCRIPT { 0 } else { ((style - 2) / 2) as usize };
        (sz, x_over_n(self.sy(6, sz), 18).0)
    }
    fn hpack(&self, list: Vec<Item>) -> BoxNode {
        crate::pack::hpack(list, Spec::Additional(0), &QUIET, self.fonts).b
    }
    fn vpack(&self, list: Vec<Item>) -> BoxNode {
        crate::pack::vpack(list, Spec::Additional(0), MAX_DIMEN, &QUIET).b
    }

    /// `fetch`: the font and its info, or None (error/warning given; the field becomes empty).
    fn fetch(&mut self, fam: u8, c: u16, sz: usize) -> Option<(FontId, Arc<Font>)> {
        match self.fam_font(sz, fam) {
            None => {
                let size = ["\\textfont", "\\scriptfont", "\\scriptscriptfont"][sz];
                self.errors.push((
                    format!("{size} {fam} is undefined (character {})", print_ascii(c)),
                    vec![
                        "Somewhere in the math formula just finished, you used the",
                        "stated character from an undefined font family. For example,",
                        "plain TeX doesn't allow \\it or \\sl in subscripts. Proceed,",
                        "and I'll try to forget that I needed that character.",
                    ],
                ));
                None
            }
            Some(id) => {
                let f = self.font(id);
                if f.has(c) {
                    Some((id, f))
                } else {
                    if self.p.tracing_lost_chars > 0 {
                        self.lost.push(format!("Missing character: There is no {} in font {}!", print_ascii(c), f.name()));
                    }
                    None
                }
            }
        }
    }

    fn char_box(&self, fid: FontId, c: u16) -> BoxNode {
        let f = self.font(fid);
        BoxNode {
            width: f.width(c) + f.italic(c),
            height: f.height(c),
            depth: f.depth(c),
            list: Arc::from(vec![Item::Char { font: fid, c }]),
            ..BoxNode::default()
        }
    }

    fn mlist(&mut self, items: &[Item], style: u8, penalties: bool) -> Vec<Item> {
        self.run(to_w(items), style, penalties)
    }

    fn clean_box(&mut self, f: &Field, s: u8) -> BoxNode {
        let q: Vec<Item> = match f {
            Field::Char { .. } => {
                let mut n = Wn::new(ORD);
                n.nucleus = f.clone();
                self.run(vec![W::N(Box::new(n))], s, false)
            }
            Field::List(l) => self.mlist(l, s, false),
            Field::Box(b) => vec![Item::Box(b.clone())],
            _ => return BoxNode::default(),
        };
        let mut x = match q.as_slice() {
            [Item::Box(b)] if b.shift == 0 => (**b).clone(),
            _ => self.hpack(q),
        };
        if x.list.len() == 2 && matches!(x.list[0], Item::Char { .. }) && matches!(x.list[1], Item::Kern { .. }) {
            x.list = Arc::from(vec![x.list[0].clone()]);
        }
        x
    }

    fn rebox(&self, b: BoxNode, w: i32) -> BoxNode {
        if b.width != w && !b.list.is_empty() {
            let b = if b.vertical { self.hpack(vec![bx(b)]) } else { b };
            let mut p: Vec<Item> = b.list.to_vec();
            if p.len() == 1 {
                if let Item::Char { font, c } = p[0] {
                    let v = self.font(font).width(c);
                    if v != b.width {
                        p.push(kern(b.width - v));
                    }
                }
            }
            let ss = Glue { width: 0, stretch: 65536, shrink: 65536, stretch_order: 1, shrink_order: 1 };
            let mut list = vec![Item::Glue { g: ss, sub: 0, leader: None }];
            list.extend(p);
            list.push(Item::Glue { g: ss, sub: 0, leader: None });
            crate::pack::hpack(list, Spec::Exactly(w), &QUIET, self.fonts).b
        } else {
            let mut b = b;
            b.width = w;
            b
        }
    }

    fn var_delimiter(&self, d: Delim, sz: usize, v: i32) -> BoxNode {
        let mut best: Option<(FontId, u16)> = None;
        let mut w = 0;
        let (mut z, mut x) = (d.small_fam, d.small_char as u16);
        let mut large = false;
        'outer: loop {
            if z != 0 || x != 0 {
                for s in (0..=sz).rev() {
                    let Some(gid) = self.fam_font(s, z) else { continue };
                    let g = self.font(gid);
                    let mut y = x;
                    loop {
                        if !g.has(y) {
                            break;
                        }
                        if g.extensible(y).is_some() {
                            best = Some((gid, y));
                            break 'outer;
                        }
                        let u = g.height(y) + g.depth(y);
                        if u > w {
                            best = Some((gid, y));
                            w = u;
                            if u >= v {
                                break 'outer;
                            }
                        }
                        match g.next_larger(y) {
                            Some(n) => y = n,
                            None => break,
                        }
                    }
                }
            }
            if large {
                break;
            }
            large = true;
            z = d.large_fam;
            x = d.large_char as u16;
        }
        let mut b = match best {
            Some((fid, c)) => {
                let f = self.font(fid);
                match f.extensible(c) {
                    Some([top, mid, bot, rep]) => {
                        let hpd = |k: u16| f.height(k) + f.depth(k);
                        let u = hpd(rep);
                        let mut w = 0;
                        for k in [bot, mid, top] {
                            if k != 0 {
                                w += hpd(k);
                            }
                        }
                        let mut n = 0;
                        if u > 0 {
                            while w < v {
                                w += u;
                                n += 1;
                                if mid != 0 {
                                    w += u;
                                }
                            }
                        }
                        // stack_into_box prepends; build bottom-up then reverse
                        let mut seq: Vec<u16> = Vec::new();
                        if bot != 0 {
                            seq.push(bot);
                        }
                        seq.extend(std::iter::repeat(rep).take(n));
                        if mid != 0 {
                            seq.push(mid);
                            seq.extend(std::iter::repeat(rep).take(n));
                        }
                        if top != 0 {
                            seq.push(top);
                        }
                        let height = seq.last().map(|&k| f.height(k)).unwrap_or(0);
                        let list: Vec<Item> = seq.iter().rev().map(|&k| bx(self.char_box(fid, k))).collect();
                        BoxNode {
                            vertical: true,
                            width: f.width(rep) + f.italic(rep),
                            height,
                            depth: w - height,
                            list: list.into(),
                            ..BoxNode::default()
                        }
                    }
                    None => self.char_box(fid, c),
                }
            }
            None => BoxNode { width: self.p.null_delimiter_space, ..BoxNode::default() },
        };
        b.shift = half(b.height - b.depth) - self.axis(sz);
        b
    }

    fn overbar(&self, b: BoxNode, k: i32, t: i32) -> BoxNode {
        self.vpack(vec![kern(t), Item::Rule { w: RUNNING, h: t, d: 0 }, kern(k), bx(b)])
    }
}

impl M<'_> {
    fn make_op(&mut self, n: &mut Wn, st: u8, sz: usize) -> i32 {
        if n.subtype == 0 && st < TEXT {
            n.subtype = 1;
        }
        let Field::Char { fam, c } = n.nucleus.clone() else { return 0 };
        let mut delta = 0;
        match self.fetch(fam, c, sz) {
            Some((_, f)) => {
                let mut c = c;
                if st < TEXT {
                    if let Some(c2) = f.next_larger(c) {
                        if f.has(c2) {
                            c = c2;
                            n.nucleus = Field::Char { fam, c };
                        }
                    }
                }
                delta = f.italic(c);
            }
            None => n.nucleus = Field::Empty,
        }
        let mut x = self.clean_box(&n.nucleus, st);
        if !empty(&n.sub) && n.subtype != 1 {
            x.width -= delta;
        }
        x.shift = half(x.height - x.depth) - self.axis(sz);
        n.nucleus = Field::Box(Arc::new(x));
        delta
    }

    /// tex.web 750–751.
    fn op_limits(&mut self, n: &mut Wn, st: u8, sz: usize, delta: i32) {
        let x = self.clean_box(&n.sup, sup_style(st));
        let y = self.clean_box(&n.nucleus, st);
        let z = self.clean_box(&n.sub, sub_style(st));
        let w = y.width.max(x.width).max(z.width);
        let mut x = self.rebox(x, w);
        let y = self.rebox(y, w);
        let mut z = self.rebox(z, w);
        x.shift = half(delta);
        z.shift = -x.shift;
        let (mut h, mut d) = (y.height, y.depth);
        let mut list = Vec::new();
        let s5 = self.ex(13, sz);
        if !empty(&n.sup) {
            let su = (self.ex(11, sz) - x.depth).max(self.ex(9, sz));
            h += s5 + x.height + x.depth + su;
            list.push(kern(s5));
            list.push(bx(x));
            list.push(kern(su));
        }
        list.push(bx(y));
        if !empty(&n.sub) {
            let sd = (self.ex(12, sz) - z.height).max(self.ex(10, sz));
            d += s5 + z.height + z.depth + sd;
            list.push(kern(sd));
            list.push(bx(z));
            list.push(kern(s5));
        }
        let v = BoxNode { vertical: true, width: w, height: h, depth: d, list: list.into(), ..BoxNode::default() };
        n.hl = vec![bx(v)];
    }

    fn make_ord(&mut self, v: &mut Vec<W>, i: usize, sz: usize) {
        loop {
            let n = wn(v, i);
            if !empty(&n.sub) || !empty(&n.sup) {
                return;
            }
            let Field::Char { fam, c } = n.nucleus else { return };
            let Some(W::N(p)) = v.get(i + 1) else { return };
            if !(ORD..=PUNCT).contains(&p.ty) {
                return;
            }
            let Field::Char { fam: pf, c: pc } = p.nucleus else { return };
            if pf != fam {
                return;
            }
            wn(v, i).nucleus = Field::TextChar { fam, c };
            let Some((_, f)) = self.fetch(fam, c, sz) else {
                wn(v, i).nucleus = Field::Empty;
                return;
            };
            match f.lig_kern(c, pc) {
                LigKern::None => return,
                LigKern::Kern(k) => {
                    v.insert(i + 1, W::Node(kern(k)));
                    return;
                }
                LigKern::Lig { op, c: rem } => {
                    let mut qc = c;
                    match op {
                        1 | 5 => qc = rem,
                        2 | 6 => {
                            let p = wn(v, i + 1);
                            p.nucleus = Field::Char { fam, c: rem };
                        }
                        3 | 7 | 11 => {
                            let mut r = Wn::new(ORD);
                            r.nucleus = if op < 11 { Field::Char { fam, c: rem } } else { Field::TextChar { fam, c: rem } };
                            v.insert(i + 1, W::N(Box::new(r)));
                        }
                        _ => {
                            let W::N(p) = v.remove(i + 1) else { return };
                            let q = wn(v, i);
                            q.sub = p.sub;
                            q.sup = p.sup;
                            qc = rem;
                        }
                    }
                    let q = wn(v, i);
                    if op > 3 {
                        q.nucleus = Field::TextChar { fam, c: qc };
                        return;
                    }
                    q.nucleus = Field::Char { fam, c: qc };
                }
            }
        }
    }

    fn make_fraction(&mut self, n: &mut Wn, st: u8, sz: usize) {
        if n.thick == DEFAULT_CODE {
            n.thick = self.ex(8, sz);
        }
        let x = self.clean_box(&Field::List(n.num.clone()), num_style(st));
        let z = self.clean_box(&Field::List(n.den.clone()), denom_style(st));
        let (x, z) = if x.width < z.width {
            let w = z.width;
            (self.rebox(x, w), z)
        } else {
            let w = x.width;
            (x, self.rebox(z, w))
        };
        let (mut up, mut down);
        if st < TEXT {
            up = self.sy(8, sz);
            down = self.sy(11, sz);
        } else {
            down = self.sy(12, sz);
            up = if n.thick != 0 { self.sy(9, sz) } else { self.sy(10, sz) };
        }
        let axis = self.axis(sz);
        let t = n.thick;
        let delta;
        if t == 0 {
            let drt = self.ex(8, sz);
            let clr = if st < TEXT { 7 * drt } else { 3 * drt };
            let dl = half(clr - ((up - x.depth) - (z.height - down)));
            if dl > 0 {
                up += dl;
                down += dl;
            }
            delta = 0;
        } else {
            let clr = if st < TEXT { 3 * t } else { t };
            delta = half(t);
            let d1 = clr - ((up - x.depth) - (axis + delta));
            let d2 = clr - ((axis - delta) - (z.height - down));
            if d1 > 0 {
                up += d1;
            }
            if d2 > 0 {
                down += d2;
            }
        }
        let mut list = Vec::new();
        let (xh, xd, xw, zh, zd) = (x.height, x.depth, x.width, z.height, z.depth);
        list.push(bx(x));
        if t == 0 {
            list.push(kern((up - xd) - (zh - down)));
        } else {
            list.push(kern((up - xd) - (axis + delta)));
            list.push(Item::Rule { w: RUNNING, h: t, d: 0 });
            list.push(kern((axis - delta) - (zh - down)));
        }
        list.push(bx(z));
        let v = BoxNode { vertical: true, width: xw, height: up + xh, depth: zd + down, list: list.into(), ..BoxNode::default() };
        let dd = if st < TEXT { self.sy(20, sz) } else { self.sy(21, sz) };
        let l = self.var_delimiter(n.delim, sz, dd);
        let r = self.var_delimiter(n.delim2, sz, dd);
        n.hl = vec![bx(self.hpack(vec![bx(l), bx(v), bx(r)]))];
    }

    fn make_radical(&mut self, n: &mut Wn, st: u8, sz: usize) {
        let x = self.clean_box(&n.nucleus, cramped(st));
        let drt = self.ex(8, sz);
        let mut clr = if st < TEXT { drt + self.sy(5, sz).abs() / 4 } else { drt + drt.abs() / 4 };
        let mut y = self.var_delimiter(n.delim, sz, x.height + x.depth + clr + drt);
        let delta = y.depth - (x.height + x.depth + clr);
        if delta > 0 {
            clr += half(delta);
        }
        y.shift = -(x.height + clr);
        let yh = y.height;
        let o = self.overbar(x, clr, yh);
        n.nucleus = Field::Box(Arc::new(self.hpack(vec![bx(y), bx(o)])));
    }

    fn make_under(&mut self, n: &mut Wn, st: u8, sz: usize) {
        let x = self.clean_box(&n.nucleus, st);
        let drt = self.ex(8, sz);
        let xh = x.height;
        let mut y = self.vpack(vec![bx(x), kern(3 * drt), Item::Rule { w: RUNNING, h: drt, d: 0 }]);
        let delta = y.height + y.depth + drt;
        y.height = xh;
        y.depth = delta - xh;
        n.nucleus = Field::Box(Arc::new(y));
    }

    fn make_math_accent(&mut self, n: &mut Wn, st: u8, sz: usize) {
        let Some((fid, f)) = self.fetch(n.accent.0, n.accent.1, sz) else { return };
        let mut c = n.accent.1;
        let mut s = 0;
        if let Field::Char { fam, c: nc } = n.nucleus {
            match self.fetch(fam, nc, sz) {
                Some((_, nf)) => {
                    let sk = self.p.skew[sz][(fam & 15) as usize];
                    if (0..=255).contains(&sk) {
                        if let LigKern::Kern(k) = nf.lig_kern(nc, sk as u16) {
                            s = k;
                        }
                    }
                }
                None => n.nucleus = Field::Empty,
            }
        }
        let mut x = self.clean_box(&n.nucleus, cramped(st));
        let w = x.width;
        let mut h = x.height;
        while let Some(y) = f.next_larger(c) {
            if !f.has(y) || f.width(y) > w {
                break;
            }
            c = y;
        }
        let xh = f.param(5);
        let mut delta = if h < xh { h } else { xh };
        if (!empty(&n.sup) || !empty(&n.sub)) && matches!(n.nucleus, Field::Char { .. }) {
            let mut inner = Wn::new(ORD);
            inner.nucleus = n.nucleus.clone();
            let mk = Noad::Simple {
                ty: ORD,
                subtype: 0,
                nucleus: inner.nucleus.clone(),
                sup: std::mem::take(&mut n.sup),
                sub: std::mem::take(&mut n.sub),
                delim: Delim::default(),
                accent: (0, 0),
            };
            n.nucleus = Field::List(Arc::from(vec![Item::Noad(Arc::new(mk))]));
            x = self.clean_box(&n.nucleus, st);
            delta = delta + x.height - h;
            h = x.height;
        }
        let mut y = self.char_box(fid, c);
        y.shift = s + half(w - y.width);
        y.width = 0;
        let xw = x.width;
        let mut yy = self.vpack(vec![bx(y), kern(-delta), bx(x)]);
        yy.width = xw;
        if yy.height < h {
            let mut l = vec![kern(h - yy.height)];
            l.extend(yy.list.iter().cloned());
            yy.list = l.into();
            yy.height = h;
        }
        n.nucleus = Field::Box(Arc::new(yy));
    }

    fn make_scripts(&mut self, n: &mut Wn, delta: i32, st: u8, sz: usize) {
        let (mut up, mut down) = if matches!(n.hl.first(), Some(Item::Char { .. })) {
            (0, 0)
        } else {
            let z = self.hpack(n.hl.clone());
            let t = if st < SCRIPT { 1 } else { 2 };
            (z.height - self.sy(18, t), z.depth + self.sy(19, t))
        };
        let x;
        if empty(&n.sup) {
            let mut b = self.clean_box(&n.sub, sub_style(st));
            b.width += self.p.script_space;
            down = down.max(self.sy(16, sz));
            let clr = b.height - (self.sy(5, sz) * 4).abs() / 5;
            down = down.max(clr);
            b.shift = down;
            x = b;
        } else {
            let mut b = self.clean_box(&n.sup, sup_style(st));
            b.width += self.p.script_space;
            let clr = if st % 2 == 1 {
                self.sy(15, sz)
            } else if st < TEXT {
                self.sy(13, sz)
            } else {
                self.sy(14, sz)
            };
            up = up.max(clr);
            up = up.max(self.sy(5, sz).abs() / 4 + b.depth);
            if empty(&n.sub) {
                b.shift = -up;
                x = b;
            } else {
                let mut y = self.clean_box(&n.sub, sub_style(st));
                y.width += self.p.script_space;
                down = down.max(self.sy(17, sz));
                let clr = 4 * self.ex(8, sz) - ((up - b.depth) - (y.height - down));
                if clr > 0 {
                    down += clr;
                    let clr = (self.sy(5, sz) * 4).abs() / 5 - (up - b.depth);
                    if clr > 0 {
                        up += clr;
                        down -= clr;
                    }
                }
                b.shift = delta;
                let k = kern((up - b.depth) - (y.height - down));
                let mut v = self.vpack(vec![bx(b), k, bx(y)]);
                v.shift = down;
                x = v;
            }
        }
        n.hl.push(bx(x));
    }

    fn run(&mut self, mut v: Vec<W>, style: u8, penalties: bool) -> Vec<Item> {
        let mut i = 0;
        let mut r: Option<usize> = None;
        let mut r_type = OP;
        let (mut max_h, mut max_d) = (0, 0);
        let mut st = style;
        let (mut sz, mut mu) = self.size_mu(st);
        while i < v.len() {
            match &mut v[i] {
                W::Style(s) => {
                    st = *s;
                    (sz, mu) = self.size_mu(st);
                    i += 1;
                    continue;
                }
                W::Choice(arms) => {
                    let chosen = arms[((st / 2) as usize).min(3)].clone();
                    v[i] = W::Style(st);
                    let ins = to_w(&chosen);
                    v.splice(i + 1..i + 1, ins);
                    i += 1;
                    continue;
                }
                W::Node(it) => {
                    let mut drop_next = false;
                    match it {
                        Item::Rule { h, d, .. } => {
                            max_h = max_h.max(*h);
                            max_d = max_d.max(*d);
                        }
                        Item::Glue { g, sub, .. } if *sub == 99 => {
                            *g = math_glue(*g, mu);
                            *sub = 0;
                        }
                        Item::Glue { sub, .. } if *sub == 98 && sz != 0 => drop_next = true,
                        Item::Kern { w, sub } if *sub == 3 => {
                            let (n, f) = mu_nf(mu);
                            *w = mu_mult(n, f, *w);
                            *sub = 1;
                        }
                        _ => {}
                    }
                    if drop_next && matches!(v.get(i + 1), Some(W::Node(Item::Glue { .. } | Item::Kern { .. }))) {
                        v.remove(i + 1);
                    }
                    i += 1;
                    continue;
                }
                W::N(_) => {}
            }
            let mut delta = 0;
            {
                let n = wn(&mut v, i);
                if n.ty == BIN && matches!(r_type, BIN | OP | REL | OPEN | PUNCT | LEFT) {
                    n.ty = ORD;
                }
            }
            let ty = wn(&mut v, i).ty;
            if matches!(ty, REL | CLOSE | PUNCT | RIGHT) && r_type == BIN {
                if let Some(ri) = r {
                    wn(&mut v, ri).ty = ORD;
                }
            }
            // 0 check dimensions, 1 done with noad, 2 convert nucleus
            let next = match ty {
                LEFT | RIGHT => 1,
                FRACTION => {
                    let mut n = wn(&mut v, i).clone();
                    self.make_fraction(&mut n, st, sz);
                    *wn(&mut v, i) = n;
                    0
                }
                OP => {
                    let mut n = wn(&mut v, i).clone();
                    delta = self.make_op(&mut n, st, sz);
                    let lim = n.subtype == 1;
                    if lim {
                        self.op_limits(&mut n, st, sz, delta);
                    }
                    *wn(&mut v, i) = n;
                    if lim { 0 } else { 2 }
                }
                ORD => {
                    self.make_ord(&mut v, i, sz);
                    2
                }
                RADICAL | OVER | UNDER | ACCENT | VCENTER => {
                    let mut n = wn(&mut v, i).clone();
                    match ty {
                        RADICAL => self.make_radical(&mut n, st, sz),
                        OVER => {
                            let x = self.clean_box(&n.nucleus, cramped(st));
                            let t = self.ex(8, sz);
                            n.nucleus = Field::Box(Arc::new(self.overbar(x, 3 * t, t)));
                        }
                        UNDER => self.make_under(&mut n, st, sz),
                        ACCENT => self.make_math_accent(&mut n, st, sz),
                        _ => {
                            if let Field::Box(b) = &n.nucleus {
                                let mut b = (**b).clone();
                                let d = b.height + b.depth;
                                b.height = self.axis(sz) + half(d);
                                b.depth = d - b.height;
                                n.nucleus = Field::Box(Arc::new(b));
                            }
                        }
                    }
                    *wn(&mut v, i) = n;
                    2
                }
                _ => 2,
            };
            if next == 2 {
                let mut n = wn(&mut v, i).clone();
                let p = match std::mem::take(&mut n.nucleus) {
                    Field::Char { fam, c } | Field::TextChar { fam, c } => {
                        let is_text = matches!(wn(&mut v, i).nucleus, Field::TextChar { .. });
                        match self.fetch(fam, c, sz) {
                            Some((fid, f)) => {
                                delta = f.italic(c);
                                let mut p = vec![Item::Char { font: fid, c }];
                                if is_text && f.param(2) != 0 {
                                    delta = 0;
                                }
                                if empty(&n.sub) && delta != 0 {
                                    p.push(kern(delta));
                                    delta = 0;
                                }
                                p
                            }
                            None => Vec::new(),
                        }
                    }
                    Field::Box(b) => vec![Item::Box(b)],
                    Field::List(l) => {
                        let h = self.mlist(&l, st, false);
                        vec![bx(self.hpack(h))]
                    }
                    _ => Vec::new(),
                };
                n.hl = p;
                if !(empty(&n.sub) && empty(&n.sup)) {
                    self.make_scripts(&mut n, delta, st, sz);
                }
                *wn(&mut v, i) = n;
            }
            if next != 1 {
                let z = self.hpack(wn(&mut v, i).hl.clone());
                max_h = max_h.max(z.height);
                max_d = max_d.max(z.depth);
            }
            r = Some(i);
            r_type = wn(&mut v, i).ty;
            if r_type == RIGHT {
                r_type = LEFT;
                st = style;
                (sz, mu) = self.size_mu(st);
            }
            i += 1;
        }
        if r_type == BIN {
            if let Some(ri) = r {
                wn(&mut v, ri).ty = ORD;
            }
        }
        // second pass
        let mut out = Vec::new();
        let mut r_type = 0u8;
        st = style;
        (sz, mu) = self.size_mu(st);
        let _ = sz;
        for i in 0..v.len() {
            let n = match &v[i] {
                W::Style(s) => {
                    st = *s;
                    (sz, mu) = self.size_mu(st);
                    continue;
                }
                W::Node(it) => {
                    out.push(it.clone());
                    continue;
                }
                W::Choice(_) => continue,
                W::N(n) => n,
            };
            let (mut t, mut pen) = (ORD, 10000);
            let mut hl = n.hl.clone();
            match n.ty {
                OP | OPEN | CLOSE | PUNCT | INNER => t = n.ty,
                BIN => {
                    t = BIN;
                    pen = self.p.bin_op_penalty;
                }
                REL => {
                    t = REL;
                    pen = self.p.rel_penalty;
                }
                FRACTION => t = INNER,
                LEFT | RIGHT => {
                    st = style;
                    (sz, mu) = self.size_mu(st);
                    let d2 = max_d + self.axis(sz);
                    let mut d1 = max_h + max_d - d2;
                    if d2 > d1 {
                        d1 = d2;
                    }
                    let mut delta = (d1 / 500).wrapping_mul(self.p.delimiter_factor);
                    let d2 = d1 + d1 - self.p.delimiter_shortfall;
                    if delta < d2 {
                        delta = d2;
                    }
                    hl = vec![bx(self.var_delimiter(n.delim, sz, delta))];
                    t = n.ty - (LEFT - OPEN);
                }
                _ => {}
            }
            if r_type > 0 {
                let x = match SPACING[((r_type - ORD) * 8 + (t - ORD)) as usize] {
                    b'1' if st < SCRIPT => 1,
                    b'2' => 1,
                    b'3' if st < SCRIPT => 2,
                    b'4' if st < SCRIPT => 3,
                    _ => 0,
                };
                if x != 0 {
                    let g = [self.p.thin, self.p.med, self.p.thick][x - 1];
                    out.push(Item::Glue { g: math_glue(g, mu), sub: 15 + x as u8, leader: None });
                }
            }
            let is_right = n.ty == RIGHT;
            out.extend(hl);
            if penalties && i + 1 < v.len() && pen < 10000 {
                let skip = match &v[i + 1] {
                    W::Node(Item::Penalty(_)) => true,
                    W::N(m) => m.ty == REL,
                    _ => false,
                };
                if !skip {
                    out.push(Item::Penalty(pen));
                }
            }
            if is_right {
                t = OPEN;
            }
            r_type = t;
        }
        out
    }
}
