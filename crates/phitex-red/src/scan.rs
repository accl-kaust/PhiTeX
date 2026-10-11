//! B3: scanning (tex.web part 26–27): numbers, dimensions, glue, keywords,
//! token lists, internal quantities via the registry, e-TeX expressions.
//!
//! Every function reads tokens through `Ctx` (`next` / `next_x` / `back` /
//! `push`) and quantities through the registry's `Kind::Quantity` /
//! `Kind::Computed` entries; nothing here keeps state between calls.
use std::sync::{Arc, OnceLock};

use crate::ctx::Ctx;
use crate::diag::error;
use crate::prim::{registry, Kind, Level, PrimId, Registry};
use crate::term::TokKind;
use crate::tok::{Tok, Toks};
use crate::val::{FontId, Glue, Key, Meaning, Val};

pub const MAX_DIMEN: i32 = 0o7777777777;
pub const INFINITY: i32 = 0o17777777777;
const UNITY: i32 = 0o200000;

// ---------------------------------------------------------------- ids

pub(crate) struct Ids {
    pub relax: Option<PrimId>,
    pub the: Option<PrimId>,
    pub unexpanded: Option<PrimId>,
    pub detokenize: Option<PrimId>,
    pub endcsname: Option<PrimId>,
    pub font: Option<PrimId>,
}

pub(crate) fn ids() -> &'static Ids {
    static I: OnceLock<Ids> = OnceLock::new();
    I.get_or_init(|| {
        let r = registry();
        Ids {
            relax: r.by_name("relax"),
            the: r.by_name("the"),
            unexpanded: r.by_name("unexpanded"),
            detokenize: r.by_name("detokenize"),
            endcsname: r.by_name("endcsname"),
            font: r.by_name("font"),
        }
    })
}

// ---------------------------------------------------------------- token tests

/// `other_token + ch`.
pub fn other(ch: char) -> Tok {
    Tok::Char { cat: 12, ch }
}

/// The category a token acts with (`cur_cmd` for characters and names let
/// to characters); `None` for anything else.
pub fn cat_of(c: &mut dyn Ctx, t: &Tok) -> Option<u8> {
    match t {
        Tok::Char { cat, .. } => Some(*cat),
        Tok::Cs(_) | Tok::Active(_) => match c.meaning(t) {
            Meaning::Char { cat, .. } => Some(cat),
            _ => None,
        },
        _ => None,
    }
}

pub fn is_spacer(c: &mut dyn Ctx, t: &Tok) -> bool {
    match t {
        Tok::Char { cat, .. } => *cat == 10,
        Tok::Cs(_) | Tok::Active(_) => cat_of(c, t) == Some(10),
        _ => false,
    }
}

/// Whether `t` means the primitive `id`.
pub fn is_prim(c: &mut dyn Ctx, t: &Tok, id: Option<PrimId>) -> bool {
    match (t, id) {
        (Tok::Cs(_) | Tok::Active(_), Some(id)) => c.meaning(t) == Meaning::Prim(id),
        _ => false,
    }
}

pub fn is_relax(c: &mut dyn Ctx, t: &Tok) -> bool {
    is_prim(c, t, ids().relax)
}

/// `get_token`: the next token unexpanded, `\noexpand` marks dropped.
pub fn get_tok(c: &mut dyn Ctx) -> Option<Tok> {
    loop {
        let t = c.next()?;
        if t != Tok::DontExpand {
            return Some(t);
        }
    }
}

fn back_opt(c: &mut dyn Ctx, t: Option<Tok>) {
    if let Some(t) = t {
        c.back(t);
    }
}

/// `get_x_token` repeated until not a spacer.
pub fn next_x_non_blank(c: &mut dyn Ctx) -> Option<Tok> {
    loop {
        let t = c.next_x()?;
        if !is_spacer(c, &t) {
            return Some(t);
        }
    }
}

/// Non-blank non-`\relax` token (`get_x_non_blank_non_relax`).
pub fn next_non_blank(c: &mut dyn Ctx) -> Option<Tok> {
    loop {
        let t = c.next_x()?;
        if !is_spacer(c, &t) && !is_relax(c, &t) {
            return Some(t);
        }
    }
}

/// `Scan an optional space`.
fn optional_space(c: &mut dyn Ctx) {
    if let Some(t) = c.next_x() {
        if !is_spacer(c, &t) {
            c.back(t);
        }
    }
}

fn mu_error(c: &mut dyn Ctx) {
    error(c, "Incompatible glue units", &["I'm going to assume that 1mu=1pt when they're mixed."]);
}

fn missing_number(c: &mut dyn Ctx) {
    error(
        c,
        "Missing number, treated as zero",
        &[
            "A number should have been here; I inserted `0'.",
            "(If you can't figure out why I needed to see a number,",
            "look up `weird error' in the index to The TeXbook.)",
        ],
    );
}

// ---------------------------------------------------------------- internal quantities

/// What an internal-quantity token is (tex.web's min_internal..max_internal).
#[derive(Clone, Copy, Debug)]
pub enum Internal {
    Prim(PrimId),
    Reg(u8, u16),
    CharGiven(u32),
    MathGiven(u32),
    Font(FontId),
}

pub fn internal_of(c: &mut dyn Ctx, t: &Tok) -> Option<Internal> {
    let m = match t {
        Tok::Cs(_) | Tok::Active(_) => c.meaning(t),
        _ => return None,
    };
    match m {
        Meaning::Prim(id) => {
            let d = registry().get(id);
            match d.kind {
                Kind::Quantity { .. } | Kind::Computed { .. } => Some(Internal::Prim(id)),
                _ if Some(id) == ids().font => Some(Internal::Prim(id)),
                _ => None,
            }
        }
        Meaning::Reg(b, n) => Some(Internal::Reg(b, n)),
        Meaning::CharGiven(n) => Some(Internal::CharGiven(n)),
        Meaning::MathCharGiven(n) => Some(Internal::MathGiven(n)),
        Meaning::Font(f) => Some(Internal::Font(f)),
        _ => None,
    }
}

/// The key of register `n` in bank `b` (0 count 1 dimen 2 skip 3 muskip 4 toks).
pub fn reg_key(b: u8, n: u16) -> Key {
    match b {
        0 => Key::Count(n),
        1 => Key::Dimen(n),
        2 => Key::Skip(n),
        3 => Key::Muskip(n),
        _ => Key::Toks(n),
    }
}

pub fn bank_level(b: u8) -> Level {
    match b {
        0 => Level::Int,
        1 => Level::Dimen,
        2 => Level::Glue,
        3 => Level::Mu,
        _ => Level::Tok,
    }
}

fn normalize(v: Val, l: Level) -> Val {
    match l {
        Level::Int | Level::Dimen => Val::Int(v.int()),
        Level::Glue | Level::Mu => Val::Glue(v.glue()),
        Level::Tok => Val::Toks(v.toks()),
        Level::Ident => match v {
            Val::Font(_) => v,
            Val::Int(n) => Val::Font(n as FontId),
            _ => Val::Font(0),
        },
    }
}

/// The current font (`cur_font`), a recorded read.
pub fn cur_font(c: &mut dyn Ctx) -> FontId {
    match c.get(&Key::CurFont) {
        Val::Font(f) => f,
        Val::Int(n) => n as FontId,
        _ => 0,
    }
}

fn fetch(c: &mut dyn Ctx, t: Tok, level: Level) -> (Val, Level) {
    let Some(q) = internal_of(c, &t) else {
        let s = crate::expand::print_cmd_chr(c, &t);
        let mut e = String::new();
        crate::expand::print_esc(c, &mut e, "the");
        error(
            c,
            &format!("You can't use `{s}' after {e}"),
            &["I'm forgetting what you said and using zero instead."],
        );
        return (Val::Int(0), if level != Level::Tok { Level::Dimen } else { Level::Int });
    };
    // A token list or font identifier where a number was wanted.
    let missing = |c: &mut dyn Ctx| {
        c.back(t);
        missing_number(c);
        (Val::Int(0), Level::Dimen)
    };
    match q {
        Internal::CharGiven(n) | Internal::MathGiven(n) => (Val::Int(n as i32), Level::Int),
        Internal::Font(f) => {
            if level != Level::Tok {
                missing(c)
            } else {
                (Val::Font(f), Level::Ident)
            }
        }
        Internal::Reg(b, n) => {
            let lv = bank_level(b);
            if lv == Level::Tok && level != Level::Tok {
                return missing(c);
            }
            let v = c.get(&reg_key(b, n));
            (normalize(v, lv), lv)
        }
        Internal::Prim(id) => {
            let d = *registry().get(id);
            if Some(id) == ids().font {
                if level != Level::Tok {
                    return missing(c);
                }
                return (Val::Font(cur_font(c)), Level::Ident);
            }
            match d.kind {
                Kind::Quantity { level: lv, key } => {
                    if lv >= Level::Ident && level != Level::Tok {
                        return missing(c);
                    }
                    let k = key(c, d.code);
                    let v = c.get(&k);
                    (normalize(v, lv), lv)
                }
                Kind::Computed { level: lv, get, .. } => {
                    if lv >= Level::Ident && level != Level::Tok {
                        return missing(c);
                    }
                    let v = get(c, d.code);
                    (normalize(v, lv), lv)
                }
                _ => (Val::Int(0), Level::Int),
            }
        }
    }
}

/// `scan_something_internal`: the value of the quantity token `t` at `level`.
/// Int/Dimen come as `Val::Int`, Glue/Mu as `Val::Glue`, Tok as `Val::Toks`,
/// Ident as `Val::Font`.
pub fn scan_internal(c: &mut dyn Ctx, t: Tok, level: Level, neg: bool) -> (Val, Level) {
    let (mut v, mut l) = fetch(c, t, level);
    // Convert to the requested level (tex.web 429).
    while l > level && l <= Level::Mu {
        match l {
            Level::Glue => {
                v = Val::Int(v.glue().width);
                l = Level::Dimen;
            }
            Level::Mu => {
                mu_error(c);
                l = Level::Glue;
            }
            Level::Dimen => l = Level::Int,
            _ => break,
        }
    }
    if neg {
        match l {
            Level::Glue | Level::Mu => {
                let g = v.glue();
                v = Val::Glue(Glue {
                    width: g.width.wrapping_neg(),
                    stretch: g.stretch.wrapping_neg(),
                    shrink: g.shrink.wrapping_neg(),
                    ..g
                });
            }
            Level::Int | Level::Dimen => v = Val::Int(v.int().wrapping_neg()),
            _ => {}
        }
    }
    (v, l)
}

// ---------------------------------------------------------------- arithmetic (tex.web 105–107)

/// `mult_and_add` (Pascal `div` truncates, as Rust's `/`).
pub fn mult_and_add(n: i32, x: i32, y: i32, max_answer: i32, arith: &mut bool) -> i32 {
    let (mut n, mut x) = (n as i64, x as i64);
    let (y, m) = (y as i64, max_answer as i64);
    if n < 0 {
        x = -x;
        n = -n;
    }
    if n == 0 {
        return 0;
    }
    if x <= (m - y) / n && -x <= (m + y) / n {
        (n * x + y) as i32
    } else {
        *arith = true;
        0
    }
}

pub fn nx_plus_y(n: i32, x: i32, y: i32, arith: &mut bool) -> i32 {
    mult_and_add(n, x, y, MAX_DIMEN, arith)
}

/// `xn_over_d`: (quotient, remainder), both with x's sign; `arith` when the
/// quotient reaches 2^30.
pub fn xn_over_d(x: i32, n: i32, d: i32, arith: &mut bool) -> (i32, i32) {
    let pos = x >= 0;
    let ax = (x as i64).abs();
    let p = ax * n as i64;
    let q = p / d as i64;
    let r = p % d as i64;
    if q >= 1 << 30 {
        *arith = true;
    }
    let q = q as i32;
    let r = r as i32;
    if pos {
        (q, r)
    } else {
        (q.wrapping_neg(), -r)
    }
}

fn add_or_sub(x: i32, y: i32, max_answer: i32, negative: bool, arith: &mut bool) -> i32 {
    let (x, mut y, m) = (x as i64, y as i64, max_answer as i64);
    if negative {
        y = -y;
    }
    let ok = if x >= 0 { y <= m - x } else { y >= -m - x };
    if ok {
        (x + y) as i32
    } else {
        *arith = true;
        0
    }
}

/// e-TeX `quotient`: n/d rounded, ties away from zero.
fn quotient(n: i32, d: i32, arith: &mut bool) -> i32 {
    if d == 0 {
        *arith = true;
        return 0;
    }
    let (mut n, mut d) = (n as i64, d as i64);
    let mut neg = false;
    if d < 0 {
        d = -d;
        neg = true;
    }
    if n < 0 {
        n = -n;
        neg = !neg;
    }
    let mut a = n / d;
    let r = n - a * d;
    if 2 * r >= d {
        a += 1;
    }
    (if neg { -a } else { a }) as i32
}

/// e-TeX `fract`: x*n/d rounded (half up on magnitudes), bounded by `max_answer`.
fn fract(x: i32, n: i32, d: i32, max_answer: i32, arith: &mut bool) -> i32 {
    if d == 0 {
        *arith = true;
        return 0;
    }
    let mut neg = false;
    let (mut x, mut n, mut d) = (x as i128, n as i128, d as i128);
    if d < 0 {
        d = -d;
        neg = true;
    }
    if x < 0 {
        x = -x;
        neg = !neg;
    } else if x == 0 {
        return 0;
    }
    if n < 0 {
        n = -n;
        neg = !neg;
    }
    let a = (2 * x * n + d) / (2 * d);
    if a > max_answer as i128 {
        *arith = true;
        return 0;
    }
    let a = a as i32;
    if neg { -a } else { a }
}

// ---------------------------------------------------------------- integers

fn scan_signs(c: &mut dyn Ctx, neg: &mut bool) -> Option<Tok> {
    loop {
        let t = next_x_non_blank(c)?;
        if t == other('-') {
            *neg = !*neg;
            continue;
        }
        if t == other('+') {
            continue;
        }
        return Some(t);
    }
}

/// `scan_int` with `radix` and the token that ended the number (`cur_tok`).
fn scan_int_ex(c: &mut dyn Ctx) -> (i32, u32, Option<Tok>) {
    let mut neg = false;
    let mut radix = 0u32;
    let first = scan_signs(c, &mut neg);
    let mut last = first;
    let mut val: i32;
    match first {
        None => {
            missing_number(c);
            val = 0;
        }
        Some(t) if t == other('`') => {
            // An alphabetic constant.
            let u = get_tok(c);
            last = u;
            let code: u32 = match u {
                Some(Tok::Char { ch, .. }) | Some(Tok::Active(ch)) => ch as u32,
                Some(Tok::Cs(cs)) => {
                    let n = c.names().name(cs);
                    let mut it = n.chars();
                    match (it.next(), it.next()) {
                        (Some(ch), None) => ch as u32,
                        _ => 256,
                    }
                }
                _ => 256,
            };
            if code > 255 {
                back_opt(c, u);
                error(
                    c,
                    "Improper alphabetic constant",
                    &["A one-character control sequence belongs after a ` mark.", "So I'm essentially inserting \\0 here."],
                );
                val = '0' as i32;
            } else {
                val = code as i32;
                if let Some(x) = c.next_x() {
                    last = Some(x);
                    if !is_spacer(c, &x) {
                        c.back(x);
                    }
                }
            }
        }
        Some(t) if internal_of(c, &t).is_some() => {
            val = scan_internal(c, t, Level::Int, false).0.int();
        }
        Some(t) => {
            radix = 10;
            let mut m: i32 = 214748364;
            let mut cur = Some(t);
            if t == other('\'') {
                radix = 8;
                m = 1 << 28;
                cur = c.next_x();
            } else if t == other('"') {
                radix = 16;
                m = 1 << 27;
                cur = c.next_x();
            }
            let mut vacuous = true;
            let mut ok = true;
            val = 0;
            loop {
                let d: i32 = match cur {
                    Some(Tok::Char { cat: 12, ch }) if ch.is_ascii_digit() && (ch as u32 - '0' as u32) < radix => {
                        ch as i32 - '0' as i32
                    }
                    Some(Tok::Char { cat: 11 | 12, ch }) if radix == 16 && ('A'..='F').contains(&ch) => {
                        ch as i32 - 'A' as i32 + 10
                    }
                    _ => break,
                };
                vacuous = false;
                if val >= m && (val > m || d > 7 || radix != 10) {
                    if ok {
                        error(
                            c,
                            "Number too big",
                            &[
                                "I can only go up to 2147483647='17777777777=\"7FFFFFFF,",
                                "so I'm using that number instead of yours.",
                            ],
                        );
                        val = INFINITY;
                        ok = false;
                    }
                } else {
                    val = val * radix as i32 + d;
                }
                cur = c.next_x();
            }
            last = cur;
            if vacuous {
                back_opt(c, cur);
                missing_number(c);
            } else if let Some(x) = cur {
                if !is_spacer(c, &x) {
                    c.back(x);
                }
            }
        }
    }
    if neg {
        val = val.wrapping_neg();
    }
    (val, radix, last)
}

pub fn scan_int(c: &mut dyn Ctx) -> i32 {
    scan_int_ex(c).0
}

fn ranged(c: &mut dyn Ctx, max: i32, msg: &str, help: &[&str]) -> i32 {
    let v = scan_int(c);
    if v < 0 || v > max {
        error(c, msg, help);
        0
    } else {
        v
    }
}

pub fn scan_char_num(c: &mut dyn Ctx) -> u16 {
    ranged(c, 255, "Bad character code", &["A character number must be between 0 and 255.", "I changed this one to zero."]) as u16
}

pub fn scan_register_num(c: &mut dyn Ctx) -> u16 {
    if c.etex() {
        ranged(c, 32767, "Bad register code", &["A register number must be between 0 and 32767.", "I changed this one to zero."]) as u16
    } else {
        ranged(c, 255, "Bad register code", &["A register number must be between 0 and 255.", "I changed this one to zero."]) as u16
    }
}

pub fn scan_eight_bit_int(c: &mut dyn Ctx) -> u8 {
    ranged(c, 255, "Bad register code", &["A register number must be between 0 and 255.", "I changed this one to zero."]) as u8
}

pub fn scan_four_bit_int(c: &mut dyn Ctx) -> u8 {
    ranged(c, 15, "Bad number", &["Since I expected to read a number between 0 and 15,", "I changed this one to zero."]) as u8
}

pub fn scan_fifteen_bit_int(c: &mut dyn Ctx) -> u32 {
    ranged(c, 0o77777, "Bad mathchar", &["A mathchar number must be between 0 and 32767.", "I changed this one to zero."]) as u32
}

pub fn scan_twenty_seven_bit_int(c: &mut dyn Ctx) -> u32 {
    ranged(
        c,
        0o777777777,
        "Bad delimiter code",
        &["A numeric delimiter code must be between 0 and 2^{27}-1.", "I changed this one to zero."],
    ) as u32
}

// ---------------------------------------------------------------- keywords

/// `scan_keyword`: lowercase `kw`, matched case-insensitively against
/// character tokens (any category), leading spaces skipped; on failure every
/// token read is put back.
pub fn scan_keyword(c: &mut dyn Ctx, kw: &str) -> bool {
    let kw: Vec<char> = kw.chars().collect();
    let mut matched: Vec<Tok> = Vec::new();
    let mut k = 0;
    loop {
        let Some(t) = c.next_x() else {
            if !matched.is_empty() {
                c.push(Arc::from(matched), TokKind::Backed);
            }
            return false;
        };
        let hit = match t {
            Tok::Char { ch, .. } => ch == kw[k] || (kw[k].is_ascii_lowercase() && ch == kw[k].to_ascii_uppercase()),
            _ => false,
        };
        if hit {
            matched.push(t);
            k += 1;
            if k == kw.len() {
                return true;
            }
        } else if !matched.is_empty() || !is_spacer(c, &t) {
            c.back(t);
            if !matched.is_empty() {
                c.push(Arc::from(matched), TokKind::Backed);
            }
            return false;
        }
    }
}

pub fn scan_optional_equals(c: &mut dyn Ctx) {
    if let Some(t) = next_x_non_blank(c) {
        if t != other('=') {
            c.back(t);
        }
    }
}

/// `scan_left_brace`: a missing one is reported and treated as read.
pub fn scan_left_brace(c: &mut dyn Ctx) {
    let t = next_non_blank(c);
    match t {
        Some(x) if cat_of(c, &x) == Some(1) => {}
        other => {
            back_opt(c, other);
            error(
                c,
                "Missing { inserted",
                &[
                    "A left brace was mandatory here, so I've put one in.",
                    "You might want to delete and/or insert some corrections",
                    "so that I will find a matching right brace soon.",
                    "(If you're confused by all this, try typing `I}' now.)",
                ],
            );
        }
    }
}

// ---------------------------------------------------------------- dimensions

fn round_decimals(digs: &[i32]) -> i32 {
    let mut a: i32 = 0;
    for &d in digs.iter().rev() {
        a = (a + d * 0o400000) / 10;
    }
    (a + 1) / 2
}

fn mag(c: &mut dyn Ctx) -> i32 {
    let k = Key::IntPar(crate::prim::ip::of("mag"));
    let m = c.int(&k);
    if m <= 0 || m > 32768 {
        error(c, "Illegal magnification has been changed to 1000", &["The magnification ratio must be between 1 and 32768."]);
        c.define(k, Val::Int(1000), true);
        1000
    } else {
        m
    }
}

fn attach_fraction(c: &mut dyn Ctx, v: i32, f: i32, arith: &mut bool) -> i32 {
    let r = if v >= 0o40000 {
        *arith = true;
        v
    } else {
        v * UNITY + f
    };
    optional_space(c);
    r
}

fn found_unit(save: i32, u: i32, f: i32, arith: &mut bool) -> i32 {
    let (y, _) = xn_over_d(u, f, 0o200000, arith);
    nx_plus_y(save, u, y, arith)
}

/// tex.web 448, with the `shortcut` (an integer already scanned).
fn scan_dimen_full(c: &mut dyn Ctx, mu: bool, inf: bool, shortcut: Option<i32>) -> (i32, u8) {
    let mut f: i32 = 0;
    let mut arith = false;
    let mut order = 0u8;
    let mut neg = false;

    let val: i32 = 'attach: {
        let mut v: i32;
        match shortcut {
            Some(x) => v = x,
            None => {
                let t = scan_signs(c, &mut neg);
                match t {
                    Some(tok) if internal_of(c, &tok).is_some() => {
                        if mu {
                            let (x, l) = scan_internal(c, tok, Level::Mu, false);
                            v = if l >= Level::Glue { x.glue().width } else { x.int() };
                            if l == Level::Mu {
                                break 'attach v;
                            }
                            if l != Level::Int {
                                mu_error(c);
                            }
                        } else {
                            let (x, l) = scan_internal(c, tok, Level::Dimen, false);
                            v = x.int();
                            if l == Level::Dimen {
                                break 'attach v;
                            }
                        }
                    }
                    tok => {
                        let point = |t: &Option<Tok>| *t == Some(other('.')) || *t == Some(other(','));
                        let mut radix = 10;
                        let cur;
                        back_opt(c, tok);
                        if point(&tok) {
                            v = 0;
                            cur = tok;
                        } else {
                            let (x, r, last) = scan_int_ex(c);
                            v = x;
                            radix = r;
                            cur = last;
                        }
                        if radix == 10 && point(&cur) {
                            let _ = get_tok(c); // the point, re-read
                            let mut digs: Vec<i32> = Vec::new();
                            let end = loop {
                                match c.next_x() {
                                    Some(Tok::Char { cat: 12, ch }) if ch.is_ascii_digit() => {
                                        if digs.len() < 17 {
                                            digs.push(ch as i32 - '0' as i32);
                                        }
                                    }
                                    other => break other,
                                }
                            };
                            f = round_decimals(&digs);
                            if let Some(x) = end {
                                if !is_spacer(c, &x) {
                                    c.back(x);
                                }
                            }
                        }
                    }
                }
            }
        }
        if v < 0 {
            neg = !neg;
            v = v.wrapping_neg();
        }
        // Units.
        if inf && scan_keyword(c, "fil") {
            order = 1;
            while scan_keyword(c, "l") {
                if order == 3 {
                    error(c, "Illegal unit of measure (replaced by filll)", &["I dddon't go any higher than filll."]);
                } else {
                    order += 1;
                }
            }
            break 'attach attach_fraction(c, v, f, &mut arith);
        }
        let save = v;
        let t = next_x_non_blank(c);
        match t {
            Some(tok) if internal_of(c, &tok).is_some() => {
                let u = if mu {
                    let (x, l) = scan_internal(c, tok, Level::Mu, false);
                    let w = if l >= Level::Glue { x.glue().width } else { x.int() };
                    if l != Level::Mu {
                        mu_error(c);
                    }
                    w
                } else {
                    scan_internal(c, tok, Level::Dimen, false).0.int()
                };
                break 'attach found_unit(save, u, f, &mut arith);
            }
            tok => back_opt(c, tok),
        }
        if !mu {
            let u = if scan_keyword(c, "em") {
                let cf = cur_font(c);
                Some(crate::font::font_dimen(c, cf, 6))
            } else if scan_keyword(c, "ex") {
                let cf = cur_font(c);
                Some(crate::font::font_dimen(c, cf, 5))
            } else if c.pdf() && scan_keyword(c, "px") {
                Some(c.int(&Key::DimenPar(crate::prim::dp::of("pdfpxdimen"))))
            } else {
                None
            };
            if let Some(u) = u {
                optional_space(c);
                break 'attach found_unit(save, u, f, &mut arith);
            }
        }
        // not_found
        if mu {
            if !scan_keyword(c, "mu") {
                error(
                    c,
                    "Illegal unit of measure (mu inserted)",
                    &[
                        "The unit of measurement in math glue must be mu.",
                        "To recover gracefully from this error, it's best to",
                        "delete the erroneous units; e.g., type `2' to delete",
                        "two letters. (See Chapter 27 of The TeXbook.)",
                    ],
                );
            }
            break 'attach attach_fraction(c, v, f, &mut arith);
        }
        if scan_keyword(c, "true") {
            let m = mag(c);
            if m != 1000 {
                let (q, r) = xn_over_d(v, 1000, m, &mut arith);
                v = q;
                let ff = (1000 * f as i64 + 0o200000 * r as i64) / m as i64;
                v = v.wrapping_add((ff / 0o200000) as i32);
                f = (ff % 0o200000) as i32;
            }
        }
        if scan_keyword(c, "pt") {
            break 'attach attach_fraction(c, v, f, &mut arith);
        }
        let (num, den): (i32, i32) = if scan_keyword(c, "in") {
            (7227, 100)
        } else if scan_keyword(c, "pc") {
            (12, 1)
        } else if scan_keyword(c, "cm") {
            (7227, 254)
        } else if scan_keyword(c, "mm") {
            (7227, 2540)
        } else if scan_keyword(c, "bp") {
            (7227, 7200)
        } else if scan_keyword(c, "dd") {
            (1238, 1157)
        } else if scan_keyword(c, "cc") {
            (14856, 1157)
        } else if scan_keyword(c, "sp") {
            optional_space(c);
            break 'attach v;
        } else {
            error(
                c,
                "Illegal unit of measure (pt inserted)",
                &[
                    "Dimensions can be in units of em, ex, in, pt, pc,",
                    "cm, mm, dd, cc, bp, or sp; but yours is a new one!",
                    "I'll assume that you meant to say pt, for printer's points.",
                    "To recover gracefully from this error, it's best to",
                    "delete the erroneous units; e.g., type `2' to delete",
                    "two letters. (See Chapter 27 of The TeXbook.)",
                ],
            );
            break 'attach attach_fraction(c, v, f, &mut arith);
        };
        let (q, r) = xn_over_d(v, num, den, &mut arith);
        v = q;
        let ff = (num as i64 * f as i64 + 0o200000 * r as i64) / den as i64;
        v = v.wrapping_add((ff / 0o200000) as i32);
        f = (ff % 0o200000) as i32;
        attach_fraction(c, v, f, &mut arith)
    };
    let mut val = val;
    if arith || val.unsigned_abs() >= 0o10000000000 {
        error(
            c,
            "Dimension too large",
            &["I can't work with sizes bigger than about 19 feet.", "Continue and I'll use the largest value I can."],
        );
        val = MAX_DIMEN;
    }
    if neg {
        val = val.wrapping_neg();
    }
    (val, order)
}

/// `scan_dimen(mu, inf, false)`: the value and its infinity order (`cur_order`).
pub fn scan_dimen(c: &mut dyn Ctx, mu: bool, inf: bool) -> (i32, u8) {
    scan_dimen_full(c, mu, inf, None)
}

/// `scan_dimen(mu, inf, true)`: units for an integer already scanned.
pub fn scan_dimen_after(c: &mut dyn Ctx, mu: bool, inf: bool, n: i32) -> (i32, u8) {
    scan_dimen_full(c, mu, inf, Some(n))
}

pub fn scan_normal_dimen(c: &mut dyn Ctx) -> i32 {
    scan_dimen_full(c, false, false, None).0
}

// ---------------------------------------------------------------- glue

/// `scan_glue(level)`, level `Glue` or `Mu`.
pub fn scan_glue(c: &mut dyn Ctx, level: Level) -> Glue {
    let mu = level == Level::Mu;
    let mut neg = false;
    let t = scan_signs(c, &mut neg);
    let w: i32 = match t {
        Some(tok) if internal_of(c, &tok).is_some() => {
            let (v, l) = scan_internal(c, tok, level, neg);
            if l >= Level::Glue {
                if l != level {
                    mu_error(c);
                }
                return v.glue();
            }
            if l == Level::Int {
                scan_dimen_full(c, mu, false, Some(v.int())).0
            } else {
                if level == Level::Mu {
                    mu_error(c);
                }
                v.int()
            }
        }
        tok => {
            back_opt(c, tok);
            let x = scan_dimen_full(c, mu, false, None).0;
            if neg { x.wrapping_neg() } else { x }
        }
    };
    let mut g = Glue { width: w, ..Glue::default() };
    if scan_keyword(c, "plus") {
        let (s, o) = scan_dimen_full(c, mu, true, None);
        g.stretch = s;
        g.stretch_order = o;
    }
    if scan_keyword(c, "minus") {
        let (s, o) = scan_dimen_full(c, mu, true, None);
        g.shrink = s;
        g.shrink_order = o;
    }
    g
}

pub fn scan_normal_glue(c: &mut dyn Ctx) -> Glue {
    scan_glue(c, Level::Glue)
}

pub fn scan_mu_glue(c: &mut dyn Ctx) -> Glue {
    scan_glue(c, Level::Mu)
}

// ---------------------------------------------------------------- token lists

/// What ended a body.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum End {
    Brace,
    Stop,
    Eof,
}

pub(crate) enum X {
    /// A token to store; `true`: it carried a `\noexpand` mark.
    Tok(Tok, bool),
    Stop,
    Eof,
}

/// The next token of an expanded text (`\edef`, `\write`, `\message`,
/// `\expanded`): expands as it goes, but `\the` (and e-TeX `\unexpanded`,
/// `\detokenize`) put their result into `out` unexpanded, and protected macros
/// are not expanded.
pub(crate) fn xnext(c: &mut dyn Ctx, out: &mut Vec<Tok>, stop: Option<Tok>) -> X {
    loop {
        let Some(t) = c.next() else { return X::Eof };
        if t == Tok::DontExpand {
            return match c.next() {
                Some(u) => X::Tok(u, true),
                None => X::Eof,
            };
        }
        if Some(t) == stop {
            return X::Stop;
        }
        if !matches!(t, Tok::Cs(_) | Tok::Active(_)) {
            return X::Tok(t, false);
        }
        let m = c.meaning(&t);
        match &m {
            Meaning::Macro(f) => {
                if f.protected {
                    return X::Tok(t, false);
                }
                crate::macros::expand_macro(c, t, f);
            }
            Meaning::Prim(id) => {
                let d = *registry().get(*id);
                match d.kind {
                    Kind::Expand(_) | Kind::If(_) => {
                        let i = ids();
                        if Some(*id) == i.the || Some(*id) == i.unexpanded || Some(*id) == i.detokenize {
                            let r = crate::expand::the_toks(c, d.code);
                            out.extend(r.iter().copied());
                        } else {
                            crate::expand::expand(c, t);
                        }
                    }
                    _ => return X::Tok(t, false),
                }
            }
            Meaning::Undefined => crate::expand::undefined(c, t),
            _ => return X::Tok(t, false),
        }
    }
}

/// The body of `scan_toks` after its left brace (tex.web 477): balanced,
/// `#n` made `Param(n)` and `##` `Param(0)` when `macro_def`.
pub(crate) fn scan_body(c: &mut dyn Ctx, macro_def: bool, xpand: bool, nparams: u8, stop: Option<Tok>) -> (Vec<Tok>, End) {
    let mut out: Vec<Tok> = Vec::new();
    let mut unbalance = 1u32;
    loop {
        let (t, marked) = if xpand {
            match xnext(c, &mut out, stop) {
                X::Tok(t, m) => (t, m),
                X::Stop => return (out, End::Stop),
                X::Eof => return (out, End::Eof),
            }
        } else {
            match get_tok(c) {
                Some(t) if Some(t) == stop => return (out, End::Stop),
                Some(t) => (t, false),
                None => return (out, End::Eof),
            }
        };
        if !marked {
            match t {
                Tok::Char { cat: 1, .. } => unbalance += 1,
                Tok::Char { cat: 2, .. } => {
                    unbalance -= 1;
                    if unbalance == 0 {
                        return (out, End::Brace);
                    }
                }
                Tok::Char { cat: 6, .. } if macro_def => {
                    let u = if xpand {
                        match xnext(c, &mut out, stop) {
                            X::Tok(u, _) => Some(u),
                            _ => None,
                        }
                    } else {
                        get_tok(c)
                    };
                    match u {
                        Some(Tok::Char { cat: 6, .. }) => out.push(Tok::Param(0)),
                        Some(Tok::Char { cat: 12, ch }) if ('1'..='9').contains(&ch) && (ch as u8 - b'0') <= nparams => {
                            out.push(Tok::Param(ch as u8 - b'0'))
                        }
                        u => {
                            back_opt(c, u);
                            error(
                                c,
                                "Illegal parameter number in definition",
                                &[
                                    "You meant to type ## instead of #, right?",
                                    "Or maybe a } was forgotten somewhere earlier, and things",
                                    "are all screwed up? I'm going to assume that you meant ##.",
                                ],
                            );
                            out.push(Tok::Param(0));
                        }
                    }
                    continue;
                }
                _ => {}
            }
        }
        out.push(t);
    }
}

/// `scan_toks(macro_def, xpand)`: (parameter text, body). With `macro_def`
/// the parameter text holds `Param(n)` for `#n`; in the `#{` form it ends
/// with that `{` token, which is appended to the body too (TeX's
/// `hash_brace`). Without `macro_def` the parameter text is empty and a left
/// brace is scanned first.
pub fn scan_toks(c: &mut dyn Ctx, macro_def: bool, xpand: bool) -> (Toks, Toks) {
    let empty: Toks = Arc::from(Vec::<Tok>::new());
    let mut params: Vec<Tok> = Vec::new();
    let mut n = 0u8;
    let mut hash_brace: Option<Tok> = None;
    if macro_def {
        loop {
            let Some(t) = get_tok(c) else { return (Arc::from(params), empty) };
            match t {
                Tok::Char { cat: 1, .. } => break,
                Tok::Char { cat: 2, .. } => {
                    error(
                        c,
                        "Missing { inserted",
                        &["Where was the left brace? You said something like `\\def\\a}',", "which I'm going to interpret as `\\def\\a{}'."],
                    );
                    return (Arc::from(params), empty);
                }
                Tok::Char { cat: 6, .. } => {
                    let Some(u) = get_tok(c) else { break };
                    if let Tok::Char { cat: 1, .. } = u {
                        hash_brace = Some(u);
                        params.push(u);
                        break;
                    }
                    if n == 9 {
                        error(
                            c,
                            "You already have nine parameters",
                            &["I'm going to ignore the # sign you just used,", "as well as the token that followed it."],
                        );
                        params.push(u);
                    } else {
                        n += 1;
                        if u != other((b'0' + n) as char) {
                            c.back(u);
                            error(
                                c,
                                "Parameters must be numbered consecutively",
                                &["I've inserted the digit you should have used after the #.", "Type `1' to delete what you did use."],
                            );
                        }
                        params.push(Tok::Param(n));
                    }
                }
                _ => params.push(t),
            }
        }
    } else {
        scan_left_brace(c);
    }
    let (mut body, _) = scan_body(c, macro_def, xpand, n, None);
    if let Some(b) = hash_brace {
        body.push(b);
    }
    (Arc::from(params), Arc::from(body))
}

/// The sentinel ending an expanded text (TeX's `\endwrite`): a name no
/// document can type.
pub(crate) fn end_write(c: &mut dyn Ctx) -> Tok {
    Tok::Cs(c.names().intern("\u{0}endwrite"))
}

/// `\edef`/`\write`/`\message` expansion of a balanced text already read
/// (tex.web 1371: the text, `}`, `\endwrite` pushed, then the body of
/// `scan_toks(false, true)`).
pub fn expand_toks(c: &mut dyn Ctx, t: &Toks) -> Toks {
    let s = end_write(c);
    c.push(Arc::from(vec![s]), TokKind::Inserted);
    let mut v: Vec<Tok> = Vec::with_capacity(t.len() + 1);
    v.extend(t.iter().copied());
    v.push(Tok::Char { cat: 2, ch: '}' });
    c.push(Arc::from(v), TokKind::Write);
    let (body, end) = scan_body(c, false, true, 0, Some(s));
    if end == End::Brace {
        // Anything left before `\endwrite` is an unbalanced write.
        let mut first = true;
        while let Some(u) = get_tok(c) {
            if u == s {
                break;
            }
            if first {
                error(
                    c,
                    "Unbalanced write command",
                    &["On this page there's a \\write with fewer real {'s than }'s.", "I can't handle that very well; good luck."],
                );
                first = false;
            }
        }
    }
    Arc::from(body)
}

/// e-TeX `scan_general_text`: a braced text, unexpanded.
pub fn scan_general_text(c: &mut dyn Ctx) -> Toks {
    scan_toks(c, false, false).1
}

// ---------------------------------------------------------------- fonts, files

/// `scan_font_ident` (tex.web 577).
pub fn scan_font_ident(c: &mut dyn Ctx) -> FontId {
    let t = next_x_non_blank(c);
    if let Some(tok) = t {
        if matches!(tok, Tok::Cs(_) | Tok::Active(_)) {
            match c.meaning(&tok) {
                Meaning::Font(f) => return f,
                Meaning::Prim(id) if Some(id) == ids().font => return cur_font(c),
                Meaning::Prim(id) => {
                    let d = *registry().get(id);
                    if let Kind::Quantity { level: Level::Ident, key } = d.kind {
                        let k = key(c, d.code);
                        return match c.get(&k) {
                            Val::Font(f) => f,
                            Val::Int(n) => n as FontId,
                            _ => 0,
                        };
                    }
                }
                _ => {}
            }
        }
    }
    back_opt(c, t);
    error(
        c,
        "Missing font identifier",
        &["I was looking for a control sequence whose", "current meaning has been defined by \\font."],
    );
    0
}

/// `scan_file_name` (tex.web 526 with TeX Live's quoted and braced names).
pub fn scan_file_name(c: &mut dyn Ctx) -> String {
    let t = next_x_non_blank(c);
    if let Some(tok @ Tok::Char { cat: 1, .. }) = t {
        // `\input{name}`: the expanded text, as a string.
        c.back(tok);
        let (_, body) = scan_toks(c, false, true);
        let s = crate::expand::show_toks(c, &body);
        return s.chars().filter(|&ch| ch != '"').collect();
    }
    let mut name = String::new();
    let mut quoted = false;
    let mut cur = t;
    loop {
        match cur {
            Some(Tok::Char { ch, .. }) if (ch as u32) < 256 => {
                if ch == ' ' && !quoted {
                    break;
                }
                if ch == '"' {
                    quoted = !quoted;
                } else {
                    name.push(ch);
                }
            }
            other => {
                back_opt(c, other);
                break;
            }
        }
        cur = c.next_x();
    }
    name
}

// ---------------------------------------------------------------- e-TeX expressions

const E_NONE: u8 = 0;
const E_ADD: u8 = 1;
const E_SUB: u8 = 2;
const E_MULT: u8 = 3;
const E_DIV: u8 = 4;
const E_SCALE: u8 = 5;

fn norm_glue(mut g: Glue) -> Glue {
    if g.stretch == 0 {
        g.stretch_order = 0;
    }
    if g.shrink == 0 {
        g.shrink_order = 0;
    }
    g
}

/// e-TeX `scan_expr` at level `l0` (Int, Dimen, Glue, Mu): Int/Dimen give
/// `Val::Int`, Glue/Mu `Val::Glue`.
pub fn scan_expr(c: &mut dyn Ctx, l0: Level) -> Val {
    let g0 = Glue::default();
    let mut l = l0;
    let mut b = false;
    // Saved (l, r, s, e, t, n) of enclosing parenthesized expressions.
    let mut stack: Vec<(Level, u8, u8, Glue, Glue, i32)> = Vec::new();
    let mut r: u8;
    let mut s: u8;
    let mut e: Glue;
    let mut t: Glue;
    let mut n: i32;
    'restart: loop {
        r = E_NONE;
        e = g0;
        s = E_NONE;
        t = g0;
        n = 0;
        'cont: loop {
            let ol = if s == E_NONE { l } else { Level::Int };
            // A factor, or a subexpression.
            let tok = next_x_non_blank(c);
            if tok == Some(other('(')) {
                stack.push((l, r, s, e, t, n));
                l = ol;
                continue 'restart;
            }
            back_opt(c, tok);
            let mut f: Glue = match ol {
                Level::Int => Glue { width: scan_int(c), ..g0 },
                Level::Dimen => Glue { width: scan_normal_dimen(c), ..g0 },
                Level::Glue => scan_glue(c, Level::Glue),
                _ => scan_glue(c, Level::Mu),
            };
            'found: loop {
                // The next operator.
                let tok = next_x_non_blank(c);
                let mut o = if tok == Some(other('+')) {
                    E_ADD
                } else if tok == Some(other('-')) {
                    E_SUB
                } else if tok == Some(other('*')) {
                    E_MULT
                } else if tok == Some(other('/')) {
                    E_DIV
                } else {
                    if stack.is_empty() {
                        if let Some(x) = tok {
                            if !is_relax(c, &x) {
                                c.back(x);
                            }
                        }
                    } else if tok != Some(other(')')) {
                        back_opt(c, tok);
                        error(c, "Missing ) inserted for expression", &["I was expecting to see `+', `-', `*', `/', or `)'. Didn't."]);
                    }
                    E_NONE
                };
                let mut arith = b;
                // f in range.
                if l == Level::Int || s > E_SUB {
                    if f.width == i32::MIN {
                        arith = true;
                        f = g0;
                    }
                } else if l == Level::Dimen {
                    if f.width.unsigned_abs() > MAX_DIMEN as u32 {
                        arith = true;
                        f = g0;
                    }
                } else if f.width.unsigned_abs() > MAX_DIMEN as u32
                    || f.stretch.unsigned_abs() > MAX_DIMEN as u32
                    || f.shrink.unsigned_abs() > MAX_DIMEN as u32
                {
                    arith = true;
                    f = g0;
                }
                // The current term.
                match s {
                    E_NONE => {
                        t = if l >= Level::Glue && o != E_NONE { norm_glue(f) } else { f };
                    }
                    E_MULT => {
                        if o == E_DIV {
                            n = f.width;
                            o = E_SCALE;
                        } else if l == Level::Int {
                            t.width = mult_and_add(t.width, f.width, 0, INFINITY, &mut arith);
                        } else if l == Level::Dimen {
                            t.width = mult_and_add(t.width, f.width, 0, MAX_DIMEN, &mut arith);
                        } else {
                            t.width = mult_and_add(t.width, f.width, 0, MAX_DIMEN, &mut arith);
                            t.stretch = mult_and_add(t.stretch, f.width, 0, MAX_DIMEN, &mut arith);
                            t.shrink = mult_and_add(t.shrink, f.width, 0, MAX_DIMEN, &mut arith);
                        }
                    }
                    E_DIV => {
                        t.width = quotient(t.width, f.width, &mut arith);
                        if l >= Level::Glue {
                            t.stretch = quotient(t.stretch, f.width, &mut arith);
                            t.shrink = quotient(t.shrink, f.width, &mut arith);
                        }
                    }
                    _ => {
                        // E_SCALE
                        if l == Level::Int {
                            t.width = fract(t.width, n, f.width, INFINITY, &mut arith);
                        } else {
                            t.width = fract(t.width, n, f.width, MAX_DIMEN, &mut arith);
                            if l >= Level::Glue {
                                t.stretch = fract(t.stretch, n, f.width, MAX_DIMEN, &mut arith);
                                t.shrink = fract(t.shrink, n, f.width, MAX_DIMEN, &mut arith);
                            }
                        }
                    }
                }
                if o > E_SUB {
                    s = o;
                } else {
                    // Evaluate the current expression.
                    s = E_NONE;
                    if r == E_NONE {
                        e = t;
                    } else if l == Level::Int {
                        e.width = add_or_sub(e.width, t.width, INFINITY, r == E_SUB, &mut arith);
                    } else if l == Level::Dimen {
                        e.width = add_or_sub(e.width, t.width, MAX_DIMEN, r == E_SUB, &mut arith);
                    } else {
                        let neg = r == E_SUB;
                        let mut ne = e;
                        ne.width = add_or_sub(e.width, t.width, MAX_DIMEN, neg, &mut arith);
                        if e.stretch_order == t.stretch_order {
                            ne.stretch = add_or_sub(e.stretch, t.stretch, MAX_DIMEN, neg, &mut arith);
                        } else if e.stretch_order < t.stretch_order && t.stretch != 0 {
                            ne.stretch = t.stretch;
                            ne.stretch_order = t.stretch_order;
                        }
                        if e.shrink_order == t.shrink_order {
                            ne.shrink = add_or_sub(e.shrink, t.shrink, MAX_DIMEN, neg, &mut arith);
                        } else if e.shrink_order < t.shrink_order && t.shrink != 0 {
                            ne.shrink = t.shrink;
                            ne.shrink_order = t.shrink_order;
                        }
                        e = norm_glue(ne);
                    }
                    r = o;
                }
                b = arith;
                if o != E_NONE {
                    continue 'cont;
                }
                if let Some((pl, pr, ps, pe, pt, pn)) = stack.pop() {
                    f = e;
                    l = pl;
                    r = pr;
                    s = ps;
                    e = pe;
                    t = pt;
                    n = pn;
                    continue 'found;
                }
                break 'restart;
            }
        }
    }
    if b {
        error(c, "Arithmetic overflow", &["I can't evaluate this expression,", "since the result is out of range."]);
        e = g0;
    }
    match l {
        Level::Int | Level::Dimen => Val::Int(e.width),
        _ => Val::Glue(e),
    }
}

const NUMEXPR: u16 = 0;
const DIMEXPR: u16 = 1;
const GLUEEXPR: u16 = 2;
const MUEXPR: u16 = 3;
const GLUESTRETCH: u16 = 4;
const GLUESHRINK: u16 = 5;
const GLUESTRETCHORDER: u16 = 6;
const GLUESHRINKORDER: u16 = 7;
const MUTOGLUE: u16 = 8;
const GLUETOMU: u16 = 9;
const ETEXVERSION: u16 = 10;

fn etex_get(c: &mut dyn Ctx, code: u16) -> Val {
    match code {
        NUMEXPR => scan_expr(c, Level::Int),
        DIMEXPR => scan_expr(c, Level::Dimen),
        GLUEEXPR => scan_expr(c, Level::Glue),
        MUEXPR => scan_expr(c, Level::Mu),
        GLUESTRETCH => Val::Int(scan_normal_glue(c).stretch),
        GLUESHRINK => Val::Int(scan_normal_glue(c).shrink),
        GLUESTRETCHORDER => Val::Int(scan_normal_glue(c).stretch_order as i32),
        GLUESHRINKORDER => Val::Int(scan_normal_glue(c).shrink_order as i32),
        MUTOGLUE => Val::Glue(scan_mu_glue(c)),
        GLUETOMU => Val::Glue(scan_normal_glue(c)),
        ETEXVERSION => Val::Int(2),
        _ => Val::Int(0),
    }
}

pub fn register(r: &mut Registry) {
    let e = |l: Level| Kind::Computed { level: l, get: etex_get, set: None };
    r.add("numexpr", NUMEXPR, e(Level::Int), 1);
    r.add("dimexpr", DIMEXPR, e(Level::Dimen), 1);
    r.add("glueexpr", GLUEEXPR, e(Level::Glue), 1);
    r.add("muexpr", MUEXPR, e(Level::Mu), 1);
    r.add("gluestretch", GLUESTRETCH, e(Level::Dimen), 1);
    r.add("glueshrink", GLUESHRINK, e(Level::Dimen), 1);
    r.add("gluestretchorder", GLUESTRETCHORDER, e(Level::Int), 1);
    r.add("glueshrinkorder", GLUESHRINKORDER, e(Level::Int), 1);
    r.add("mutoglue", MUTOGLUE, e(Level::Glue), 1);
    r.add("gluetomu", GLUETOMU, e(Level::Mu), 1);
    r.add("eTeXversion", ETEXVERSION, e(Level::Int), 1);
}
