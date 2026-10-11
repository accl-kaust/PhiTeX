//! B4: conditionals as `Select` over thunks: every `\if…`, `\ifcase`,
//! `\unless`, e-TeX/pdfTeX conditionals, `\else` `\or` `\fi`.
//!
//! The open conditionals are a value in env (`Key::Conds`, innermost last),
//! defined globally (conditionals are not group-local). A true test continues
//! with the arm; a false one passes the untaken arm raw (`Ctx::skip_raw`), its
//! span being the untaken thunk, and continues after the `\else`/`\or`/`\fi`.
//! `Cond::state`: 0 test (TeX's `if_code` limit), 1 then-arm (`else_code`),
//! 2 else-arm (`fi_code`), 3 `\ifcase` arm (`or_code`).
use std::sync::Arc;

use crate::ctx::Ctx;
use crate::env::Mode;
use crate::prim::{registry, Kind, Level, PrimId, Registry};
use crate::scan;
use crate::term::{Span, TokKind};
use crate::tok::Tok;
use crate::val::{Cond, Key, Meaning, Val};

// `\if` codes (tex.web 487, e-TeX, pdfTeX): `\currentiftype` is code + 1.
pub const IF_CHAR: u16 = 0;
pub const IF_CAT: u16 = 1;
pub const IF_INT: u16 = 2;
pub const IF_DIM: u16 = 3;
pub const IF_ODD: u16 = 4;
pub const IF_VMODE: u16 = 5;
pub const IF_HMODE: u16 = 6;
pub const IF_MMODE: u16 = 7;
pub const IF_INNER: u16 = 8;
pub const IF_VOID: u16 = 9;
pub const IF_HBOX: u16 = 10;
pub const IF_VBOX: u16 = 11;
pub const IFX: u16 = 12;
pub const IF_EOF: u16 = 13;
pub const IF_TRUE: u16 = 14;
pub const IF_FALSE: u16 = 15;
pub const IF_CASE: u16 = 16;
pub const IF_DEF: u16 = 17;
pub const IF_CS: u16 = 18;
pub const IF_FONT_CHAR: u16 = 19;
pub const IF_IN_CSNAME: u16 = 20;
pub const IF_PDF_PRIMITIVE: u16 = 21;
pub const IF_PDF_ABS_NUM: u16 = 22;
pub const IF_PDF_ABS_DIM: u16 = 23;
/// Added to `Cond::kind` for an `\unless` test.
pub const UNLESS: u16 = 32;

// `\fi` `\else` `\or` codes (tex.web 489).
pub const FI: u16 = 2;
pub const ELSE: u16 = 3;
pub const OR: u16 = 4;

pub fn register(r: &mut Registry) {
    let tex: &[(&'static str, u16)] = &[
        ("if", IF_CHAR), ("ifcat", IF_CAT), ("ifnum", IF_INT), ("ifdim", IF_DIM),
        ("ifodd", IF_ODD), ("ifvmode", IF_VMODE), ("ifhmode", IF_HMODE), ("ifmmode", IF_MMODE),
        ("ifinner", IF_INNER), ("ifvoid", IF_VOID), ("ifhbox", IF_HBOX), ("ifvbox", IF_VBOX),
        ("ifx", IFX), ("ifeof", IF_EOF), ("iftrue", IF_TRUE), ("iffalse", IF_FALSE),
        ("ifcase", IF_CASE),
    ];
    for &(n, c) in tex {
        r.add(n, c, Kind::If(test), 0);
    }
    r.add("fi", FI, Kind::Expand(fi_or_else), 0);
    r.add("else", ELSE, Kind::Expand(fi_or_else), 0);
    r.add("or", OR, Kind::Expand(fi_or_else), 0);
    r.add("ifdefined", IF_DEF, Kind::If(test), 1);
    r.add("ifcsname", IF_CS, Kind::If(test), 1);
    r.add("iffontchar", IF_FONT_CHAR, Kind::If(test), 1);
    r.add("unless", 0, Kind::Expand(unless), 1);
    for (n, c) in [("currentiflevel", 0u16), ("currentiftype", 1), ("currentifbranch", 2)] {
        r.add(n, c, Kind::Computed { level: Level::Int, get: current_if, set: None }, 1);
    }
    r.add("ifincsname", IF_IN_CSNAME, Kind::If(test), 2);
    r.add("ifpdfprimitive", IF_PDF_PRIMITIVE, Kind::If(test), 2);
    r.add("ifpdfabsnum", IF_PDF_ABS_NUM, Kind::If(test), 2);
    r.add("ifpdfabsdim", IF_PDF_ABS_DIM, Kind::If(test), 2);
}

// ---- the open conditionals

fn conds(ctx: &mut dyn Ctx) -> Vec<Cond> {
    match ctx.get(&Key::Conds) {
        Val::Conds(c) => c.to_vec(),
        _ => Vec::new(),
    }
}

fn set_conds(ctx: &mut dyn Ctx, c: Vec<Cond>) {
    ctx.define(Key::Conds, Val::Conds(Arc::from(c)), true);
}

/// `change_if_limit` on the conditional at `idx`.
fn set_state(ctx: &mut dyn Ctx, idx: usize, state: u8) {
    let mut c = conds(ctx);
    if let Some(x) = c.get_mut(idx) {
        x.state = state;
        set_conds(ctx, c);
    }
}

/// `pop_cond`: drops the innermost conditional.
fn pop(ctx: &mut dyn Ctx) {
    let mut c = conds(ctx);
    c.pop();
    set_conds(ctx, c);
}

/// TeX's `if_limit` for a state.
fn limit(state: u8) -> u16 {
    match state {
        0 => 1, // if_code
        1 => ELSE,
        2 => FI,
        _ => OR,
    }
}

// ---- passing an untaken arm

/// What a token means to the skipper: `Some(None)` an `\if…` (nests),
/// `Some(Some(code))` `\fi`/`\else`/`\or`, `None` anything else.
fn cond_meaning(ctx: &mut dyn Ctx, t: &Tok) -> Option<Option<u16>> {
    let Meaning::Prim(id) = ctx.meaning(t) else { return None };
    let d = registry().get(id);
    match d.kind {
        Kind::If(_) => Some(None),
        Kind::Expand(_) => match d.name {
            "fi" => Some(Some(FI)),
            "else" => Some(Some(ELSE)),
            "or" => Some(Some(OR)),
            _ => None,
        },
        _ => None,
    }
}

/// `pass_text`: skips raw to the next `\fi`/`\else`/`\or` at this level
/// (nesting on every conditional meaning) and consumes it. `None`: the input
/// ended first ("Incomplete \if").
fn pass_text(ctx: &mut dyn Ctx, spans: &mut Vec<Span>) -> Option<u16> {
    let mut depth = 0i32;
    loop {
        if ctx.halted() {
            return None;
        }
        let mut hit: Option<Tok> = None;
        // Only a control sequence or active character can be a conditional;
        // the meaning is looked up here, outside the raw skip.
        let sk = ctx.skip_raw(
            &mut |t: &Tok, _d: i32| {
                if matches!(t, Tok::Cs(_) | Tok::Active(_)) {
                    hit = Some(*t);
                    true
                } else {
                    false
                }
            },
            &mut |_t: &Tok| 0,
        );
        spans.extend(sk);
        let t = hit?;
        match cond_meaning(ctx, &t) {
            Some(None) => depth += 1,
            Some(Some(code)) => {
                if depth == 0 {
                    return Some(code);
                }
                if code == FI {
                    depth -= 1;
                }
            }
            None => {}
        }
    }
}

fn incomplete(ctx: &mut dyn Ctx, line: u32) {
    let msg = format!("Incomplete \\if; all text was ignored after line {line}");
    crate::diag::error(
        ctx,
        &msg,
        &[
            "A forbidden control sequence occurred in skipped text.",
            "This kind of error happens when you say `\\if...' and forget",
            "the matching `\\fi'. I've inserted a `\\fi'; this might work.",
        ],
    );
}

// ---- the select

/// A conditional primitive `prim` (code `code`) was met while expanding.
/// Returns the test's value and the span of the arm passed unreduced (if any).
pub fn begin_if(ctx: &mut dyn Ctx, prim: PrimId, code: u16) -> (bool, Option<Span>) {
    begin(ctx, prim, code, false)
}

/// `\unless` followed by `prim`.
pub fn begin_unless(ctx: &mut dyn Ctx, prim: PrimId, code: u16) -> (bool, Option<Span>) {
    begin(ctx, prim, code, true)
}

/// The skipped spans as one span (first text's extent).
fn hull(spans: &[Span]) -> Option<Span> {
    let f = spans.first()?;
    let to = spans.iter().filter(|s| s.text == f.text).map(|s| s.to).max().unwrap_or(f.to);
    Some(Span { text: f.text, from: f.from, to })
}

fn begin(ctx: &mut dyn Ctx, prim: PrimId, code: u16, negate: bool) -> (bool, Option<Span>) {
    let mut skipped: Vec<Span> = Vec::new();
    let line = ctx.line_no().max(0) as u32;
    let mut c = conds(ctx);
    let idx = c.len();
    c.push(Cond { kind: if negate { code + UNLESS } else { code }, state: 0, line });
    set_conds(ctx, c);

    if code == IF_CASE {
        let mut n = scan::scan_int(ctx);
        while n != 0 {
            let Some(k) = pass_text(ctx, &mut skipped) else {
                incomplete(ctx, line);
                truncate(ctx, idx);
                return (false, hull(&skipped));
            };
            if conds(ctx).len() == idx + 1 {
                if k == OR {
                    n -= 1;
                } else {
                    common_ending(ctx, idx, k);
                    return (false, hull(&skipped));
                }
            } else if k == FI {
                pop(ctx); // a conditional opened during the test
            }
        }
        set_state(ctx, idx, 3);
        return (true, hull(&skipped));
    }

    let b = match registry().get(prim).kind {
        Kind::If(f) => f(ctx, code),
        _ => test(ctx, code),
    } != negate;
    if b {
        set_state(ctx, idx, 1);
        return (true, None);
    }
    // The false arm: pass it; it stays an unreduced thunk.
    loop {
        let Some(k) = pass_text(ctx, &mut skipped) else {
            incomplete(ctx, line);
            truncate(ctx, idx);
            return (false, hull(&skipped));
        };
        if conds(ctx).len() == idx + 1 {
            if k != OR {
                common_ending(ctx, idx, k);
                return (false, hull(&skipped));
            }
            crate::diag::error(
                ctx,
                "Extra \\or",
                &["I'm ignoring this; it doesn't match any \\if."],
            );
        } else if k == FI {
            pop(ctx);
        }
    }
}

fn common_ending(ctx: &mut dyn Ctx, idx: usize, k: u16) {
    if k == FI {
        pop(ctx);
    } else {
        set_state(ctx, idx, 2);
    }
}

/// The input ended inside a skipped arm: the conditional is closed.
fn truncate(ctx: &mut dyn Ctx, idx: usize) {
    let mut c = conds(ctx);
    c.truncate(idx);
    set_conds(ctx, c);
}

/// `\fi` `\else` `\or`.
fn fi_or_else(ctx: &mut dyn Ctx, code: u16) {
    let c = conds(ctx);
    let lim = c.last().map_or(0, |x| limit(x.state));
    if code > lim {
        if c.last().is_some_and(|x| x.state == 0) {
            // Still in the test: insert `\relax` before this token (insert_relax).
            let names = ctx.names();
            let me = Tok::Cs(names.intern(match code {
                FI => "fi",
                ELSE => "else",
                _ => "or",
            }));
            let relax = Tok::Cs(names.intern("relax"));
            ctx.push(Arc::from(vec![relax, me]), TokKind::Inserted);
        } else {
            let msg = match code {
                FI => "Extra \\fi",
                ELSE => "Extra \\else",
                _ => "Extra \\or",
            };
            crate::diag::error(ctx, msg, &["I'm ignoring this; it doesn't match any \\if."]);
        }
        return;
    }
    let line = c.last().map_or(0, |x| x.line);
    let mut k = code;
    while k != FI {
        match pass_text(ctx, &mut Vec::new()) {
            Some(n) => k = n,
            None => {
                incomplete(ctx, line);
                break;
            }
        }
    }
    pop(ctx);
}

/// `\unless`.
fn unless(ctx: &mut dyn Ctx, _code: u16) {
    let Some(t) = ctx.next() else { return };
    if let Meaning::Prim(id) = ctx.meaning(&t) {
        let d = registry().get(id);
        if matches!(d.kind, Kind::If(_)) && d.code != IF_CASE {
            begin(ctx, id, d.code, true);
            return;
        }
    }
    crate::diag::error(
        ctx,
        "You can't use `\\unless' before this token",
        &["Continue, and I'll forget that it ever happened."],
    );
    ctx.back(t);
}

/// `\currentiflevel` `\currentiftype` `\currentifbranch`.
fn current_if(ctx: &mut dyn Ctx, code: u16) -> Val {
    let c = conds(ctx);
    Val::Int(match code {
        0 => c.len() as i32,
        1 => c.last().map_or(0, |x| {
            if x.kind >= UNLESS {
                -(i32::from(x.kind - UNLESS) + 1)
            } else {
                i32::from(x.kind) + 1
            }
        }),
        _ => match c.last().map(|x| x.state) {
            Some(1 | 3) => 1,
            Some(2) => -1,
            _ => 0,
        },
    })
}

// ---- the tests

/// `get_x_token_or_active_char` then (cmd, chr) as `\if`/`\ifcat` compare them.
fn char_code(ctx: &mut dyn Ctx) -> (u8, u32) {
    let Some(t) = ctx.next_x() else { return (16, 256) };
    match t {
        Tok::Char { cat, ch } => (cat, ch as u32),
        Tok::Active(ch) => match ctx.meaning(&t) {
            Meaning::Char { cat, ch } => (cat, ch as u32),
            // An active char that stayed (\noexpand'd or unexpandable).
            _ => (13, ch as u32),
        },
        Tok::Cs(_) => match ctx.meaning(&t) {
            Meaning::Char { cat, ch } => (cat, ch as u32),
            _ => (16, 256),
        },
        _ => (16, 256),
    }
}

/// The relation of `\ifnum` `\ifdim` `\ifpdfabsnum` `\ifpdfabsdim`.
fn relation(ctx: &mut dyn Ctx, code: u16) -> char {
    let t = loop {
        match ctx.next_x() {
            Some(Tok::Char { cat: 10, .. }) => continue,
            other => break other,
        }
    };
    if let Some(Tok::Char { cat: 12, ch: ch @ ('<' | '=' | '>') }) = t {
        return ch;
    }
    let name = match code {
        IF_INT => "\\ifnum",
        IF_DIM => "\\ifdim",
        IF_PDF_ABS_NUM => "\\ifpdfabsnum",
        _ => "\\ifpdfabsdim",
    };
    crate::diag::error(
        ctx,
        &format!("Missing = inserted for {name}"),
        &["I was expecting to see `<', `=', or `>'. Didn't."],
    );
    if let Some(t) = t {
        ctx.back(t);
    }
    '='
}

fn compare(a: i64, r: char, b: i64) -> bool {
    match r {
        '<' => a < b,
        '>' => a > b,
        _ => a == b,
    }
}

fn token_meaning(ctx: &mut dyn Ctx, t: Option<Tok>) -> Meaning {
    match t {
        None => Meaning::Undefined,
        Some(Tok::Char { cat, ch }) => Meaning::Char { cat, ch },
        Some(t) => ctx.meaning(&t),
    }
}

/// Every test, by code (registered as `Kind::If`).
fn test(ctx: &mut dyn Ctx, code: u16) -> bool {
    match code {
        IF_CHAR | IF_CAT => {
            let (m1, n1) = char_code(ctx);
            let (m2, n2) = char_code(ctx);
            if code == IF_CHAR { n1 == n2 } else { m1 == m2 }
        }
        IF_INT | IF_PDF_ABS_NUM => {
            let mut a = i64::from(scan::scan_int(ctx));
            let r = relation(ctx, code);
            let mut b = i64::from(scan::scan_int(ctx));
            if code == IF_PDF_ABS_NUM {
                a = a.abs();
                b = b.abs();
            }
            compare(a, r, b)
        }
        IF_DIM | IF_PDF_ABS_DIM => {
            let mut a = i64::from(scan::scan_normal_dimen(ctx));
            let r = relation(ctx, code);
            let mut b = i64::from(scan::scan_normal_dimen(ctx));
            if code == IF_PDF_ABS_DIM {
                a = a.abs();
                b = b.abs();
            }
            compare(a, r, b)
        }
        IF_ODD => scan::scan_int(ctx) % 2 != 0,
        IF_VMODE => matches!(ctx.mode(), Mode::Vertical { .. }),
        IF_HMODE => matches!(ctx.mode(), Mode::Horizontal { .. }),
        IF_MMODE => matches!(ctx.mode(), Mode::Math { .. }),
        IF_INNER => matches!(
            ctx.mode(),
            Mode::Vertical { inner: true } | Mode::Horizontal { inner: true } | Mode::Math { display: false }
        ),
        IF_VOID | IF_HBOX | IF_VBOX => {
            let n = scan::scan_register_num(ctx);
            match ctx.get(&Key::Box(n)) {
                Val::Box(Some(b)) => match code {
                    IF_HBOX => !b.vertical,
                    IF_VBOX => b.vertical,
                    _ => false,
                },
                _ => code == IF_VOID,
            }
        }
        IFX => {
            let a = ctx.next();
            let b = ctx.next();
            let ma = token_meaning(ctx, a);
            let mb = token_meaning(ctx, b);
            ma == mb
        }
        IF_EOF => {
            let n = scan::scan_four_bit_int(ctx);
            match ctx.get(&Key::ReadStream(n)) {
                Val::Stream(s) => s.next > s.data.len(),
                _ => true,
            }
        }
        IF_TRUE => true,
        IF_FALSE => false,
        IF_CASE => scan::scan_int(ctx) == 0,
        IF_DEF => {
            let t = ctx.next();
            !matches!(token_meaning(ctx, t), Meaning::Undefined)
        }
        IF_CS => {
            let end = registry().by_name("endcsname");
            let mut name = String::new();
            loop {
                let Some(t) = ctx.next_x() else { break };
                match t {
                    Tok::Char { ch, .. } => name.push(ch),
                    Tok::Cs(_) if end.is_some() && ctx.meaning(&t) == Meaning::Prim(end.unwrap()) => break,
                    _ => {
                        crate::diag::error(
                            ctx,
                            "Missing \\endcsname inserted",
                            &[
                                "The control sequence marked <to be read again> should",
                                "not appear between \\csname and \\endcsname.",
                            ],
                        );
                        ctx.back(t);
                        break;
                    }
                }
            }
            let cs = ctx.names().intern(&name);
            !matches!(ctx.meaning(&Tok::Cs(cs)), Meaning::Undefined)
        }
        IF_FONT_CHAR => {
            let f = scan::scan_font_ident(ctx);
            let c = scan::scan_char_num(ctx);
            crate::font::fonts().font(f).has(c)
        }
        IF_IN_CSNAME => ctx.get(&Key::Other(crate::prim::tag::EXPAND, IN_CSNAME)).int() > 0,
        IF_PDF_PRIMITIVE => {
            let Some(t) = ctx.next() else { return false };
            match (t, ctx.meaning(&t)) {
                (Tok::Cs(cs), Meaning::Prim(id)) => *ctx.names().name(cs) == *registry().get(id).name,
                _ => false,
            }
        }
        _ => false,
    }
}

/// `Key::Other(tag::EXPAND, IN_CSNAME)`: the `\csname` nesting depth (B3 defines it).
pub const IN_CSNAME: u32 = 0;
