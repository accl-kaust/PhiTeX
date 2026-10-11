//! C2: `\hbox` `\vbox` `\vtop` `\box` `\copy` `\lastbox` `\vsplit`
//! `\setbox`, `\raise` …, `\unhbox` …, `\leaders`, `\wd` `\ht` `\dp`, `\shipout`'s box.
//!
//! A box under construction is a group (its kind) plus a list scope; what
//! TeX keeps in `saved(0..2)` (box context, spec) is a local definition of
//! `Key::Other(TAG, 0)` in the box group, read back by `package` at `}`.
use std::sync::Arc;

use crate::ctx::Ctx;
use crate::env::{GroupKind, Mode};
use crate::item::{BoxNode, Item, RUNNING};
use crate::pack::{self, Params, Spec, MAX_DIMEN};
use crate::prim::{dp, gp, ip, tp, Kind, Level, Registry};
use crate::scan;
use crate::term::{Emit, TokKind};
use crate::tok::Tok;
use crate::val::{Glue, Key, Meaning, Val};

/// Module tag for `Key::Other` (boxes).
pub const TAG: u8 = 11;
const IGNORE_DEPTH: i32 = -65_536_000;
const AWFUL_BAD: i32 = 0o7777777777;
const INF_BAD: i32 = 10_000;
const INF_PENALTY: i32 = 10_000;
const EJECT_PENALTY: i32 = -10_000;
const DEPLORABLE: i32 = 100_000;

fn saved_key() -> Key {
    Key::Other(TAG, 0)
}
/// e-TeX `\pagediscards` (written by the page builder as a `Val::Box` vlist).
pub fn page_discards_key() -> Key {
    Key::Other(TAG, 1)
}
/// e-TeX `\splitdiscards` (written by `\vsplit` as a `Val::Box` vlist).
pub fn split_discards_key() -> Key {
    Key::Other(TAG, 2)
}

const MAKE_BOX: [(&str, u16); 7] =
    [("box", 0), ("copy", 1), ("lastbox", 2), ("vsplit", 3), ("vtop", 4), ("vbox", 5), ("hbox", 6)];
const MOVES: [(&str, u16); 4] = [("moveright", 0), ("moveleft", 1), ("lower", 2), ("raise", 3)];
const UNPACK: [(&str, u16, u8); 6] = [
    ("unhbox", 0, 0),
    ("unhcopy", 1, 0),
    ("unvbox", 2, 0),
    ("unvcopy", 3, 0),
    ("pagediscards", 4, 1),
    ("splitdiscards", 5, 1),
];

pub fn register(r: &mut Registry) {
    for (n, code) in MAKE_BOX {
        r.add(n, code, Kind::Command(make_box_cmd), 0);
    }
    for (n, code) in MOVES {
        r.add(n, code, Kind::Command(move_cmd), 0);
    }
    for (n, code) in [("leaders", 0u16), ("cleaders", 1), ("xleaders", 2)] {
        r.add(n, code, Kind::Command(leaders_cmd), 0);
    }
    for (n, code, e) in UNPACK {
        r.add(n, code, Kind::Command(unpackage_cmd), e);
    }
    for (n, code) in [("wd", 0u16), ("ht", 1), ("dp", 2)] {
        r.add(n, code, Kind::Computed { level: Level::Dimen, get: box_dim_get, set: Some(box_dim_set) }, 0);
    }
}

/// Box context (tex.web 1071): 0 plain, shift, `\setbox` n, leaders, shipout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxContext { Plain, Shift(i32), SetBox(u16, bool), Leaders(u8), Ship }

// ---------------------------------------------------------------- helpers

fn is_v(m: Mode) -> bool {
    matches!(m, Mode::Vertical { .. })
}
fn is_h(m: Mode) -> bool {
    matches!(m, Mode::Horizontal { .. })
}
fn is_m(m: Mode) -> bool {
    matches!(m, Mode::Math { .. })
}
fn mode_name(m: Mode) -> &'static str {
    match m {
        Mode::Vertical { inner: false } => "vertical mode",
        Mode::Vertical { inner: true } => "internal vertical mode",
        Mode::Horizontal { inner: false } => "horizontal mode",
        Mode::Horizontal { inner: true } => "restricted horizontal mode",
        Mode::Math { display: true } => "display math mode",
        Mode::Math { display: false } => "math mode",
    }
}
fn you_cant(c: &mut dyn Ctx, name: &str, help: &[&str]) {
    let m = c.mode();
    let msg = format!("You can't use `\\{name}' in {}", mode_name(m));
    crate::diag::error(c, &msg, help);
}
fn report_illegal_case(c: &mut dyn Ctx, name: &str) {
    you_cant(
        c,
        name,
        &[
            "Sorry, but I'm not programmed to handle this case;",
            "I'll just pretend that you didn't ask for it.",
            "If you're in the wrong mode, you might be able to",
            "return to the right one by typing `I}' or `I$' or `I\\par'.",
        ],
    );
}
/// The command token itself, to put back (`back_input`).
fn self_tok(c: &mut dyn Ctx, name: &str) -> Tok {
    Tok::Cs(c.names().intern(name))
}
fn head_for_vmode(c: &mut dyn Ctx, t: Tok) {
    if let Mode::Horizontal { inner: true } = c.mode() {
        crate::groups::off_save(c, t);
    } else {
        c.back(t);
        let par = Tok::Cs(c.names().intern("par"));
        c.push(Arc::from(vec![par]), TokKind::Inserted);
    }
}
fn insert_dollar_sign(c: &mut dyn Ctx, t: Tok) {
    c.back(t);
    c.push(Arc::from(vec![Tok::Char { cat: 3, ch: '$' }]), TokKind::Inserted);
    crate::diag::error(
        c,
        "Missing $ inserted",
        &["I've inserted a begin-math/end-math symbol since I think", "you left one out. Proceed, with fingers crossed."],
    );
}
fn prim_name(c: &mut dyn Ctx, t: &Tok) -> Option<(&'static str, u16)> {
    match c.meaning(t) {
        Meaning::Prim(id) => {
            let d = crate::prim::registry().get(id);
            Some((d.name, d.code))
        }
        _ => None,
    }
}
fn is_space(t: &Tok) -> bool {
    matches!(t, Tok::Char { cat: 10, .. })
}
/// The next non-blank non-`\relax` unexpandable token.
fn next_non_blank_non_relax(c: &mut dyn Ctx) -> Option<(Tok, Option<(&'static str, u16)>)> {
    loop {
        let t = c.next_x()?;
        if is_space(&t) {
            continue;
        }
        let p = prim_name(c, &t);
        if matches!(p, Some(("relax", _))) {
            continue;
        }
        return Some((t, p));
    }
}
fn with_shift(b: Arc<BoxNode>, s: i32) -> Arc<BoxNode> {
    if b.shift == s {
        b
    } else {
        let mut n = (*b).clone();
        n.shift = s;
        Arc::new(n)
    }
}
fn box_reg(c: &mut dyn Ctx, n: u16) -> Option<Arc<BoxNode>> {
    match c.get(&Key::Box(n)) {
        Val::Box(b) => b,
        _ => None,
    }
}
/// Changes a box register where it lives, like TeX's direct `box(n):=…`
/// (no save entry, not affected by `\global`).
fn change_box(c: &mut dyn Ctx, n: u16, b: Option<Arc<BoxNode>>) {
    c.replace(Key::Box(n), Val::Box(b));
}

/// What packing reads (after the box group is closed, as TeX does).
pub fn read_params(c: &mut dyn Ctx) -> Params {
    Params {
        hbadness: c.int(&Key::IntPar(ip::of("hbadness"))),
        vbadness: c.int(&Key::IntPar(ip::of("vbadness"))),
        hfuzz: c.int(&Key::DimenPar(dp::of("hfuzz"))),
        vfuzz: c.int(&Key::DimenPar(dp::of("vfuzz"))),
        overfull_rule: c.int(&Key::DimenPar(dp::of("overfullrule"))),
    }
}

/// Emits a packing report with its place.
pub fn report(c: &mut dyn Ctx, p: &pack::Packed) {
    if let Some(r) = &p.report {
        let line = c.line_no();
        let msg = format!("{}) detected at line {line}", r.msg.trim_end_matches(')'));
        c.emit(Emit::Diag(Arc::new(crate::diag::Diag { msg, help: r.help.clone(), line })));
    }
}

/// `normal_paragraph` (tex.web 1070).
fn normal_paragraph(c: &mut dyn Ctx) {
    let k = Key::IntPar(ip::of("looseness"));
    if c.int(&k) != 0 {
        c.define(k, Val::Int(0), false);
    }
    let k = Key::DimenPar(dp::of("hangindent"));
    if c.int(&k) != 0 {
        c.define(k, Val::Int(0), false);
    }
    let k = Key::IntPar(ip::of("hangafter"));
    if c.int(&k) != 1 {
        c.define(k, Val::Int(1), false);
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

fn encode(b: BoxContext, s: Spec) -> Val {
    let (k, x, y) = match b {
        BoxContext::Plain => (0, 0, 0),
        BoxContext::Shift(s) => (1, s, 0),
        BoxContext::SetBox(n, g) => (2, i32::from(n), i32::from(g)),
        BoxContext::Leaders(l) => (3, i32::from(l), 0),
        BoxContext::Ship => (4, 0, 0),
    };
    let (sk, sv) = match s {
        Spec::Exactly(v) => (0, v),
        Spec::Additional(v) => (1, v),
    };
    Val::Ints(Arc::from(vec![k, x, y, sk, sv]))
}
fn decode(v: &Val) -> (BoxContext, Spec) {
    match v {
        Val::Ints(a) if a.len() == 5 => {
            let b = match a[0] {
                1 => BoxContext::Shift(a[1]),
                2 => BoxContext::SetBox(a[1] as u16, a[2] != 0),
                3 => BoxContext::Leaders(a[1] as u8),
                4 => BoxContext::Ship,
                _ => BoxContext::Plain,
            };
            let s = if a[3] == 0 { Spec::Exactly(a[4]) } else { Spec::Additional(a[4]) };
            (b, s)
        }
        _ => (BoxContext::Plain, Spec::Additional(0)),
    }
}

/// `scan_spec`'s keywords: `to` or `spread` a dimen.
fn scan_spec(c: &mut dyn Ctx) -> Spec {
    if scan::scan_keyword(c, "to") {
        Spec::Exactly(scan::scan_normal_dimen(c))
    } else if scan::scan_keyword(c, "spread") {
        Spec::Additional(scan::scan_normal_dimen(c))
    } else {
        Spec::Additional(0)
    }
}

/// `scan_rule_spec` (tex.web 463).
fn scan_rule_spec(c: &mut dyn Ctx, hrule: bool) -> Item {
    let (mut w, mut h, mut d) = if hrule { (RUNNING, 26214, 0) } else { (26214, RUNNING, RUNNING) };
    loop {
        if scan::scan_keyword(c, "width") {
            w = scan::scan_normal_dimen(c);
        } else if scan::scan_keyword(c, "height") {
            h = scan::scan_normal_dimen(c);
        } else if scan::scan_keyword(c, "depth") {
            d = scan::scan_normal_dimen(c);
        } else {
            break;
        }
    }
    Item::Rule { w, h, d }
}

// ---------------------------------------------------------------- commands

fn make_box_cmd(c: &mut dyn Ctx, code: u16) {
    make_box(c, BoxContext::Plain, code);
}

fn move_cmd(c: &mut dyn Ctx, code: u16) {
    let m = c.mode();
    let hmove = code < 2;
    if (hmove && is_v(m)) || (!hmove && !is_v(m)) {
        let d = scan::scan_normal_dimen(c);
        let s = if code & 1 == 1 { d.wrapping_neg() } else { d };
        begin_box(c, BoxContext::Shift(s));
    } else {
        report_illegal_case(c, MOVES[code as usize].0);
    }
}

fn leaders_cmd(c: &mut dyn Ctx, code: u16) {
    begin_box(c, BoxContext::Leaders(code as u8));
}

/// `\setbox` after its prefixes (called by `assign.rs`): `n`, `=`, a box.
pub fn set_box(c: &mut dyn Ctx, global: bool) {
    let n = scan::scan_register_num(c);
    scan::scan_optional_equals(c);
    begin_box(c, BoxContext::SetBox(n, global));
}

/// `scan_box` (tex.web 1084): the next box-making command, or a rule for leaders.
pub fn begin_box(c: &mut dyn Ctx, ctx: BoxContext) {
    let Some((t, p)) = next_non_blank_non_relax(c) else { return };
    if let Some((name, code)) = p {
        if MAKE_BOX.iter().any(|(n, k)| *n == name && *k == code) {
            make_box(c, ctx, code);
            return;
        }
        if matches!(ctx, BoxContext::Leaders(_)) && (name == "hrule" || name == "vrule") {
            let r = scan_rule_spec(c, name == "hrule");
            box_end_item(c, ctx, Some(r));
            return;
        }
    }
    c.back(t);
    crate::diag::error(
        c,
        "A <box> was supposed to be here",
        &[
            "I was expecting to see \\hbox or \\vbox or \\copy or \\box or",
            "something like that. So you might find something missing in",
            "your output. But keep trying; you can fix this later.",
        ],
    );
}

/// `begin_box` proper (tex.web 1079): code 0 `\box` 1 `\copy` 2 `\lastbox`
/// 3 `\vsplit` 4 `\vtop` 5 `\vbox` 6 `\hbox`.
fn make_box(c: &mut dyn Ctx, ctx: BoxContext, code: u16) {
    match code {
        0 => {
            let n = scan::scan_register_num(c);
            let b = box_reg(c, n);
            if b.is_some() {
                change_box(c, n, None);
            }
            box_end(c, ctx, b);
        }
        1 => {
            let n = scan::scan_register_num(c);
            let b = box_reg(c, n);
            box_end(c, ctx, b);
        }
        2 => {
            let m = c.mode();
            let mut b = None;
            if is_m(m) {
                you_cant(c, "lastbox", &["Sorry; this \\lastbox will be void."]);
            } else {
                match c.tail() {
                    Some(Item::Box(t)) => {
                        c.emit(Emit::RemoveTail(3));
                        b = Some(with_shift(t, 0));
                    }
                    None if m == (Mode::Vertical { inner: false }) => you_cant(
                        c,
                        "lastbox",
                        &["Sorry...I usually can't take things from the current page.", "This \\lastbox will therefore be void."],
                    ),
                    _ => {}
                }
            }
            box_end(c, ctx, b);
        }
        3 => {
            let n = scan::scan_register_num(c);
            if !scan::scan_keyword(c, "to") {
                crate::diag::error(
                    c,
                    "Missing `to' inserted",
                    &["I'm working on `\\vsplit<box number> to <dimen>';", "will look for the <dimen> next."],
                );
            }
            let h = scan::scan_normal_dimen(c);
            let b = vsplit(c, n, h);
            box_end(c, ctx, b);
        }
        _ => {
            let hbox = code == 6;
            let kind = if hbox {
                if matches!(ctx, BoxContext::Plain | BoxContext::Shift(_)) && is_v(c.mode()) {
                    GroupKind::AdjustedHbox
                } else {
                    GroupKind::Hbox
                }
            } else if code == 5 {
                GroupKind::Vbox
            } else {
                GroupKind::Vtop
            };
            let spec = scan_spec(c);
            c.open_group(kind);
            c.define(saved_key(), encode(ctx, spec), false);
            scan::scan_left_brace(c);
            if !hbox {
                normal_paragraph(c);
            }
            let (every, tk) = if hbox {
                c.open_list(Mode::Horizontal { inner: true });
                c.define(Key::SpaceFactor, Val::Int(1000), false);
                ("everyhbox", TokKind::EveryHbox)
            } else {
                c.open_list(Mode::Vertical { inner: true });
                c.define(Key::PrevDepth, Val::Int(IGNORE_DEPTH), false);
                ("everyvbox", TokKind::EveryVbox)
            };
            let t = c.get(&Key::TokPar(tp::of(every))).toks();
            if !t.is_empty() {
                c.push(t, tk);
            }
        }
    }
}

/// A box group ends (called by `groups::right_brace`, the group still open).
pub fn package(c: &mut dyn Ctx, kind: GroupKind) {
    let d = c.int(&Key::DimenPar(dp::of("boxmaxdepth")));
    let (ctx, spec) = decode(&c.get(&saved_key()));
    let items = c.close_list();
    c.close_group();
    let p = read_params(c);
    let (b, adjust) = match kind {
        GroupKind::Hbox | GroupKind::AdjustedHbox => {
            let r = pack::hpack_with(items, spec, &p, crate::font::fonts(), kind == GroupKind::AdjustedHbox);
            report(c, &r);
            (r.b, r.adjust)
        }
        _ => {
            let r = pack::vpack(items, spec, d, &p);
            report(c, &r);
            let mut b = r.b;
            if kind == GroupKind::Vtop {
                let h = match b.list.first() {
                    Some(Item::Box(f)) => f.height,
                    Some(Item::Rule { h, .. }) => *h,
                    _ => 0,
                };
                b.depth = b.depth.wrapping_sub(h).wrapping_add(b.height);
                b.height = h;
            }
            (b, Vec::new())
        }
    };
    box_end(c, ctx, Some(Arc::new(b)));
    for it in adjust {
        c.item(it);
    }
}

pub fn box_end(c: &mut dyn Ctx, ctx: BoxContext, b: Option<Arc<crate::item::BoxNode>>) {
    box_end_item(c, ctx, b.map(Item::Box));
}

/// `box_end` (tex.web 1075) for a box or (leaders only) a rule.
fn box_end_item(c: &mut dyn Ctx, ctx: BoxContext, it: Option<Item>) {
    match ctx {
        BoxContext::Plain | BoxContext::Shift(_) => {
            let s = if let BoxContext::Shift(s) = ctx { s } else { 0 };
            let Some(Item::Box(b)) = it else { return };
            let b = with_shift(b, s);
            let m = c.mode();
            if is_v(m) {
                crate::list::append_to_vlist(c, b);
            } else if is_h(m) {
                c.define(Key::SpaceFactor, Val::Int(1000), false);
                c.item(Item::Box(b));
            } else {
                let n = crate::math::box_noad(b);
                c.item(n);
            }
        }
        BoxContext::SetBox(n, g) => {
            let b = match it {
                Some(Item::Box(b)) => Some(b),
                _ => None,
            };
            c.define(Key::Box(n), Val::Box(b), g);
        }
        BoxContext::Leaders(k) => {
            if let Some(it) = it {
                leaders_end(c, k, it);
            }
        }
        BoxContext::Ship => {
            if let Some(Item::Box(b)) = it {
                crate::output::ship_out(c, b);
            }
        }
    }
}

/// tex.web 1078: the glue after `\leaders<box or rule>`.
fn leaders_end(c: &mut dyn Ctx, k: u8, it: Item) {
    let Some((t, p)) = next_non_blank_non_relax(c) else { return };
    let v = is_v(c.mode());
    let glue = p.and_then(|(name, _)| {
        let (vert, kind) = match name {
            "hfil" => (false, 0),
            "hfill" => (false, 1),
            "hss" => (false, 2),
            "hfilneg" => (false, 3),
            "hskip" => (false, 4),
            "vfil" => (true, 0),
            "vfill" => (true, 1),
            "vss" => (true, 2),
            "vfilneg" => (true, 3),
            "vskip" => (true, 4),
            _ => return None,
        };
        (vert == v).then_some(kind)
    });
    match glue {
        Some(kind) => {
            let g = match kind {
                0 => Glue { stretch: 65536, stretch_order: 1, ..Glue::default() },
                1 => Glue { stretch: 65536, stretch_order: 2, ..Glue::default() },
                2 => Glue { stretch: 65536, stretch_order: 1, shrink: 65536, shrink_order: 1, ..Glue::default() },
                3 => Glue { stretch: -65536, stretch_order: 1, ..Glue::default() },
                _ => scan::scan_glue(c, Level::Glue),
            };
            c.item(Item::Glue { g, sub: 100 + k, leader: Some(Arc::new(it)) });
        }
        None => {
            c.back(t);
            crate::diag::error(
                c,
                "Leaders not followed by proper glue",
                &[
                    "You should say `\\leaders <box or rule><hskip or vskip>'.",
                    "I found the <box or rule>, but there's no suitable",
                    "<hskip or vskip>, so I'm ignoring these leaders.",
                ],
            );
        }
    }
}

// ---------------------------------------------------------------- unpackage

fn unpackage_cmd(c: &mut dyn Ctx, code: u16) {
    let m = c.mode();
    let vertical = code >= 2;
    let name = UNPACK[code as usize].0;
    if vertical {
        if is_h(m) {
            let t = self_tok(c, name);
            return head_for_vmode(c, t);
        }
        if is_m(m) {
            let t = self_tok(c, name);
            return insert_dollar_sign(c, t);
        }
    } else if is_v(m) {
        let t = self_tok(c, name);
        c.back(t);
        crate::list::new_graf(c, true);
        return;
    }
    if code >= 4 {
        let k = if code == 4 { page_discards_key() } else { split_discards_key() };
        if let Val::Box(Some(b)) = c.get(&k) {
            for it in b.list.iter() {
                c.item(it.clone());
            }
            c.define(k, Val::Box(None), true);
        }
        return;
    }
    let n = scan::scan_register_num(c);
    let Some(b) = box_reg(c, n) else { return };
    if is_m(m) || (is_v(m) && !b.vertical) || (is_h(m) && b.vertical) {
        crate::diag::error(
            c,
            "Incompatible list can't be unboxed",
            &[
                "Sorry, Pandora. (You sneaky devil.)",
                "I refuse to unbox an \\hbox in vertical mode or vice versa.",
                "And I can't open any boxes in math mode.",
            ],
        );
        return;
    }
    if code & 1 == 0 {
        change_box(c, n, None);
    }
    for it in b.list.iter() {
        c.item(it.clone());
    }
}

// ---------------------------------------------------------------- \wd \ht \dp

fn box_dim_get(c: &mut dyn Ctx, code: u16) -> Val {
    let n = scan::scan_register_num(c);
    Val::Int(match box_reg(c, n) {
        Some(b) => match code {
            0 => b.width,
            1 => b.height,
            _ => b.depth,
        },
        None => 0,
    })
}

/// `\wd n = dimen`: scans the register number; the value is scanned here
/// when `val` is `Val::Void`. Changes the box where it lives (not `\global`).
fn box_dim_set(c: &mut dyn Ctx, code: u16, val: Val, _global: bool) {
    let n = scan::scan_register_num(c);
    let v = match val {
        Val::Void => {
            scan::scan_optional_equals(c);
            scan::scan_normal_dimen(c)
        }
        v => v.int(),
    };
    if let Some(b) = box_reg(c, n) {
        let mut nb = (*b).clone();
        match code {
            0 => nb.width = v,
            1 => nb.height = v,
            _ => nb.depth = v,
        }
        change_box(c, n, Some(Arc::new(nb)));
    }
}

// ---------------------------------------------------------------- \vsplit

/// The result of `vert_break`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VertBreak {
    /// Index of the best break (`list.len()`: at the end, TeX's null).
    pub at: usize,
    pub best_height_plus_depth: i32,
    /// How many infinitely shrinkable glues were made finite (one error each).
    pub infinite_shrink: u32,
}

fn precedes_break(it: &Item) -> bool {
    matches!(
        it,
        Item::Box(_)
            | Item::Rule { .. }
            | Item::Ins(_)
            | Item::Mark { .. }
            | Item::Adjust(_)
            | Item::Lig { .. }
            | Item::Disc(_)
            | Item::Whatsit(_)
            | Item::Char { .. }
    )
}

/// `vert_break(p, h, d)` (tex.web 970–977). Infinite shrinkage in glue is
/// made finite in `list` itself, as TeX does.
pub fn vert_break(list: &mut [Item], h: i32, d: i32) -> VertBreak {
    let len = list.len();
    let mut cur_height = 0i32;
    let mut prev_dp = 0i32;
    let mut st = [0i32; 4];
    let mut shrink = 0i32;
    let mut least = AWFUL_BAD;
    let mut out = VertBreak { at: len, best_height_plus_depth: 0, infinite_shrink: 0 };
    enum Step {
        NotFound,
        Update,
        Check(i32),
    }
    for i in 0..=len {
        let step = if i == len {
            Step::Check(EJECT_PENALTY)
        } else {
            match &list[i] {
                Item::Box(b) => {
                    cur_height = cur_height.wrapping_add(prev_dp).wrapping_add(b.height);
                    prev_dp = b.depth;
                    Step::NotFound
                }
                Item::Rule { h: rh, d: rd, .. } => {
                    cur_height = cur_height.wrapping_add(prev_dp).wrapping_add(*rh);
                    prev_dp = *rd;
                    Step::NotFound
                }
                Item::Glue { .. } => {
                    let prev = if i == 0 { &list[0] } else { &list[i - 1] };
                    if precedes_break(prev) { Step::Check(0) } else { Step::Update }
                }
                Item::Kern { .. } => {
                    if matches!(list.get(i + 1), Some(Item::Glue { .. })) { Step::Check(0) } else { Step::Update }
                }
                Item::Penalty(p) => Step::Check(*p),
                _ => Step::NotFound,
            }
        };
        let update = match step {
            Step::NotFound => false,
            Step::Update => true,
            Step::Check(pi) => {
                if pi < INF_PENALTY {
                    let b = if cur_height < h {
                        if st[1] != 0 || st[2] != 0 || st[3] != 0 {
                            0
                        } else {
                            crate::arith::badness(h - cur_height, st[0])
                        }
                    } else if cur_height.wrapping_sub(h) > shrink {
                        AWFUL_BAD
                    } else {
                        crate::arith::badness(cur_height - h, shrink)
                    };
                    let b = if b < AWFUL_BAD {
                        if pi <= EJECT_PENALTY {
                            pi
                        } else if b < INF_BAD {
                            b + pi
                        } else {
                            DEPLORABLE
                        }
                    } else {
                        b
                    };
                    if b <= least {
                        out.at = i;
                        least = b;
                        out.best_height_plus_depth = cur_height.wrapping_add(prev_dp);
                    }
                    if b == AWFUL_BAD || pi <= EJECT_PENALTY {
                        return out;
                    }
                }
                i < len && matches!(list[i], Item::Glue { .. } | Item::Kern { .. })
            }
        };
        if update {
            let w = match &mut list[i] {
                Item::Kern { w, .. } => *w,
                Item::Glue { g, .. } => {
                    let so = (g.stretch_order & 3) as usize;
                    st[so] = st[so].wrapping_add(g.stretch);
                    shrink = shrink.wrapping_add(g.shrink);
                    if g.shrink_order != 0 && g.shrink != 0 {
                        out.infinite_shrink += 1;
                        g.shrink_order = 0;
                    }
                    g.width
                }
                _ => 0,
            };
            cur_height = cur_height.wrapping_add(prev_dp).wrapping_add(w);
            prev_dp = 0;
        }
        if prev_dp > d {
            cur_height = cur_height.wrapping_add(prev_dp - d);
            prev_dp = d;
        }
    }
    out
}

/// `prune_page_top(p, s)` (tex.web 968, e-TeX): drops glue, kern and penalty
/// before the first box or rule, inserting `\splittopskip` glue before it.
/// Returns the list and the discarded items (kept only when `save`).
pub fn prune_page_top(list: Vec<Item>, split_top_skip: Glue, save: bool) -> (Vec<Item>, Vec<Item>) {
    let mut out = Vec::with_capacity(list.len() + 1);
    let mut disc = Vec::new();
    let mut it = list.into_iter();
    while let Some(x) = it.next() {
        match x {
            Item::Box(_) | Item::Rule { .. } => {
                let ht = match &x {
                    Item::Box(b) => b.height,
                    Item::Rule { h, .. } => *h,
                    _ => 0,
                };
                let mut g = split_top_skip;
                if g.width > ht {
                    g.width -= ht;
                } else {
                    g.width = 0;
                }
                out.push(Item::Glue { g, sub: gp::of("splittopskip") as u8 + 1, leader: None });
                out.push(x);
                out.extend(it);
                break;
            }
            Item::Glue { .. } | Item::Kern { .. } | Item::Penalty(_) => {
                if save {
                    disc.push(x);
                }
            }
            _ => out.push(x),
        }
    }
    (out, disc)
}

/// `vsplit(n, h)` (tex.web 977, e-TeX marks classes and `\splitdiscards`).
pub fn vsplit(c: &mut dyn Ctx, n: u16, h: i32) -> Option<Arc<BoxNode>> {
    let v = box_reg(c, n);
    // split marks are cleared (class 0 and every class the box holds)
    let mut classes: Vec<u16> = vec![0];
    if let Some(v) = &v {
        for it in v.list.iter() {
            if let Item::Mark { class, .. } = it {
                if !classes.contains(class) {
                    classes.push(*class);
                }
            }
        }
    }
    let empty = Val::Toks(Arc::from(Vec::<Tok>::new()));
    for &cl in &classes {
        c.define(Key::Mark(cl, 3), empty.clone(), true);
        c.define(Key::Mark(cl, 4), empty.clone(), true);
    }
    let etex = c.etex();
    if etex {
        c.define(split_discards_key(), Val::Box(None), true);
    }
    let v = v?;
    if !v.vertical {
        crate::diag::error(
            c,
            "\\vsplit needs a \\vbox",
            &["The box you are trying to split is an \\hbox.", "I can't split such a box, so I'll leave it alone."],
        );
        return None;
    }
    let smd = c.int(&Key::DimenPar(dp::of("splitmaxdepth")));
    let mut list: Vec<Item> = v.list.to_vec();
    let vb = vert_break(&mut list, h, smd);
    for _ in 0..vb.infinite_shrink {
        crate::diag::error(
            c,
            "Infinite glue shrinkage found in box being split",
            &[
                "The box you are \\vsplitting contains some infinitely",
                "shrinkable glue, e.g., `\\vss' or `\\vskip 0pt minus 1fil'.",
                "Such glue doesn't belong there; but you can safely proceed,",
                "since the offensive shrinkability has been made finite.",
            ],
        );
    }
    let rest = list.split_off(vb.at);
    let mut seen: Vec<u16> = Vec::new();
    for it in &list {
        if let Item::Mark { class, toks } = it {
            if !seen.contains(class) {
                seen.push(*class);
                c.define(Key::Mark(*class, 3), Val::Toks(toks.clone()), true);
            }
            c.define(Key::Mark(*class, 4), Val::Toks(toks.clone()), true);
        }
    }
    let save = etex && c.int(&Key::IntPar(ip::of("savingvdiscards"))) > 0;
    let sts = c.get(&Key::GluePar(gp::of("splittopskip"))).glue();
    let (rest, disc) = prune_page_top(rest, sts, save);
    if save && !disc.is_empty() {
        let db = BoxNode { vertical: true, list: disc.into(), ..BoxNode::default() };
        c.define(split_discards_key(), Val::Box(Some(Arc::new(db))), true);
    }
    let p = read_params(c);
    let rem = if rest.is_empty() {
        None
    } else {
        Some(Arc::new(pack::vpack(rest, Spec::Additional(0), MAX_DIMEN, &p).b))
    };
    change_box(c, n, rem);
    let r = pack::vpack(list, Spec::Exactly(h), smd, &p);
    report(c, &r);
    Some(Arc::new(r.b))
}
