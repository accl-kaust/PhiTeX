//! B5: `{` `}` `\begingroup` `\endgroup` `\aftergroup`, and dispatch of a
//! group's end by its kind to its owner (boxes, math, align, output, insert, disc…).
//!
//! A group is a scope in env (`ctx.open_group` / `ctx.close_group`); nothing
//! here keeps state. `\bgroup`/`\egroup` are `\let` to `{`/`}`: the reducer
//! calls [`left_brace`] / [`right_brace`] for any token whose meaning is a
//! character of category 1 / 2.
use std::sync::Arc;

use crate::ctx::Ctx;
use crate::diag::error;
use crate::env::{GroupKind, Mode};
use crate::prim::{Kind, Level, Registry};
use crate::term::TokKind;
use crate::tok::Tok;
use crate::val::Val;

pub fn register(r: &mut Registry) {
    r.add("begingroup", 0, Kind::Command(cmd), 0);
    r.add("endgroup", 1, Kind::Command(cmd), 0);
    r.add("aftergroup", 2, Kind::Command(cmd), 0);
    r.add("currentgrouplevel", 0, Kind::Computed { level: Level::Int, get: current_group, set: None }, 1);
    r.add("currentgrouptype", 1, Kind::Computed { level: Level::Int, get: current_group, set: None }, 1);
}

fn cmd(c: &mut dyn Ctx, code: u16) {
    match code {
        0 => c.open_group(GroupKind::SemiSimple),
        1 => end_group(c),
        _ => {
            // tex.web 1271: get_token; save_for_after(cur_tok)
            if let Some(t) = c.next() {
                c.after_group(t);
            }
        }
    }
}

/// e-TeX `\currentgrouplevel` (0) / `\currentgrouptype` (1).
fn current_group(c: &mut dyn Ctx, code: u16) -> Val {
    if code == 0 { Val::Int(c.group_depth() as i32) } else { Val::Int(group_code(c.group_kind())) }
}

/// e-TeX's group codes (`\currentgrouptype`).
pub fn group_code(k: GroupKind) -> i32 {
    match k {
        GroupKind::Bottom => 0,
        GroupKind::Simple => 1,
        GroupKind::Hbox => 2,
        GroupKind::AdjustedHbox => 3,
        GroupKind::Vbox => 4,
        GroupKind::Vtop => 5,
        GroupKind::Align => 6,
        GroupKind::NoAlign => 7,
        GroupKind::Output => 8,
        GroupKind::Math => 9,
        GroupKind::Disc => 10,
        GroupKind::Insert => 11,
        GroupKind::Vcenter => 12,
        GroupKind::MathChoice => 13,
        GroupKind::SemiSimple => 14,
        GroupKind::MathShift => 15,
        GroupKind::MathLeft => 16,
    }
}

/// `{` (or `\bgroup`) was met (tex.web 1063): a simple group, or in math
/// mode a math group (C5's).
pub fn left_brace(c: &mut dyn Ctx) {
    if matches!(c.mode(), Mode::Math { .. }) {
        crate::math::math_left_brace(c);
    } else {
        c.open_group(GroupKind::Simple);
    }
}

/// `\endgroup` (tex.web 1065).
fn end_group(c: &mut dyn Ctx) {
    if c.group_kind() == GroupKind::SemiSimple {
        c.close_group();
    } else {
        let t = Tok::Cs(c.names().intern("endgroup"));
        off_save(c, t);
    }
}

fn brace(cat: u8, ch: char) -> Tok {
    Tok::Char { cat, ch }
}

/// `off_save` (tex.web 1064): a group-ending token `t` that does not match
/// the innermost group. At the bottom level it is an "Extra" error and `t`
/// is dropped; otherwise `t` is put back and the missing closer inserted
/// before it.
pub fn off_save(c: &mut dyn Ctx, t: Tok) {
    let kind = c.group_kind();
    if kind == GroupKind::Bottom {
        let s = show(c, &t);
        error(c, &format!("Extra {s}"), &["Things are pretty mixed up, but I think the worst is over."]);
        return;
    }
    c.back(t);
    let (ins, what): (Vec<Tok>, &str) = match kind {
        GroupKind::SemiSimple => (vec![Tok::Cs(c.names().intern("endgroup"))], "\\endgroup"),
        GroupKind::MathShift => (vec![Tok::Char { cat: 3, ch: '$' }], "$"),
        GroupKind::MathLeft => {
            (vec![Tok::Cs(c.names().intern("right")), Tok::Char { cat: 12, ch: '.' }], "\\right. ")
        }
        _ => (vec![brace(2, '}')], "}"),
    };
    c.push(Arc::from(ins), TokKind::Inserted);
    error(
        c,
        &format!("Missing {what} inserted"),
        &[
            "I've inserted something that you may have forgotten. (See the",
            "<inserted text> above.) With luck, this will get me unwedged. But if you",
            "really didn't forget anything, try typing `2' now; then",
            "my insertion and my current dilemma will both disappear.",
        ],
    );
}

/// `}` (or `\egroup`) was met (tex.web 1068): the group ends as its kind says.
pub fn right_brace(c: &mut dyn Ctx) {
    let kind = c.group_kind();
    match kind {
        GroupKind::Simple => c.close_group(),
        GroupKind::Bottom => error(
            c,
            "Too many }'s",
            &["You've closed more groups than you opened.", "Such booboos are generally harmless, so keep going."],
        ),
        GroupKind::SemiSimple | GroupKind::MathShift | GroupKind::MathLeft => error(
            c,
            "Extra }, or forgotten \\endgroup, $, or \\right",
            &[
                "I've deleted a group-closing symbol because it seems to be",
                "spurious, as in `$x}$'. But perhaps the } is legitimate and",
                "you forgot something else, as in `\\hbox{$x}'. In such cases",
                "the way to recover is to insert both the forgotten and the",
                "deleted material, e.g., by typing `I$}'.",
            ],
        ),
        GroupKind::Hbox | GroupKind::AdjustedHbox => crate::boxes::package(c, kind),
        GroupKind::Vbox | GroupKind::Vtop => {
            // tex.web 1085: end_graf; package(0)
            if matches!(c.mode(), Mode::Horizontal { .. }) {
                crate::list::end_graf(c);
            }
            crate::boxes::package(c, kind);
        }
        GroupKind::Vcenter => {
            if matches!(c.mode(), Mode::Horizontal { .. }) {
                crate::list::end_graf(c);
            }
            crate::math::math_group_end(c, kind);
        }
        GroupKind::Math | GroupKind::MathChoice => crate::math::math_group_end(c, kind),
        GroupKind::Align | GroupKind::NoAlign => crate::align::align_group_end(c, kind),
        GroupKind::Output => crate::output::output_group_end(c),
        GroupKind::Insert | GroupKind::Disc => crate::list::group_end(c, kind),
    }
}

fn show(c: &dyn Ctx, t: &Tok) -> String {
    match t {
        Tok::Cs(n) => format!("\\{}", c.names().name(*n)),
        Tok::Active(ch) | Tok::Char { ch, .. } => ch.to_string(),
        Tok::Param(n) => format!("#{n}"),
        Tok::DontExpand => "\\notexpanded:".into(),
    }
}
