//! C6: `\halign` `\valign`: preamble, cells as thunks with templates pushed
//! around them, `\omit` `\span` `\noalign` `\cr` `\crcr` `\tabskip`, setting.
//!
//! The alignment in progress is a value in env, `Key::Other(tag::ALIGN, n)`
//! (n = nesting level, itself kept at `Key::Other(tag::ALIGN, 0)`, defined
//! locally in the alignment's outer group so it reverts at the alignment's end).
//! A cell is just input: the u template, the cell text up to its `&`/`\span`/
//! `\cr`, which is replaced by the v template ending in `\endtemplate`; that
//! token's node packs the cell (tex.web 768–812). No align_state counter: a
//! tab ends a cell when the innermost brace level is the cell's group.
use std::sync::Arc;

use crate::ctx::Ctx;
use crate::env::{GroupKind, Mode};
use crate::item::{BoxNode, GlueSet, Item, RUNNING};
use crate::pack::{hpack, vpack, Params, Spec};
use crate::prim::{dp, gp, ip, registry, tag, tp, Kind, Registry};
use crate::term::{Emit, TokKind};
use crate::tok::{Tok, Toks};
use crate::val::{Datum, Glue, Key, Meaning, Val};

/// The internal end-of-template primitive's name.
pub const ENDTEMPLATE: &str = "endtemplate";

const NULL_FLAG: i32 = -0o10_000_000_000;
const MAX_DIMEN: i32 = 0o7_777_777_777;
const IGNORE_DEPTH: i32 = -65_536_000;

pub fn register(r: &mut Registry) {
    r.add("halign", 0, Kind::Command(align_cmd), 0);
    r.add("valign", 1, Kind::Command(align_cmd), 0);
    r.add("span", 0, Kind::Command(car_ret_cmd), 0);
    r.add("cr", 1, Kind::Command(car_ret_cmd), 0);
    r.add("crcr", 2, Kind::Command(car_ret_cmd), 0);
    r.add("omit", 0, Kind::Command(omit_cmd), 0);
    r.add("noalign", 0, Kind::Command(noalign_cmd), 0);
    r.add(ENDTEMPLATE, 0, Kind::Command(endv_cmd), 0);
}

/// An unset box (tex.web 159): a cell (in a row's list) or a row (in the
/// alignment's list) before the column widths are known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unset {
    /// false: packed by `hpack`; true: by `vpack`.
    pub vertical: bool,
    pub width: i32,
    pub height: i32,
    pub depth: i32,
    /// Columns spanned minus one (`span_count`).
    pub span: u16,
    pub stretch: i32,
    pub stretch_order: u8,
    pub shrink: i32,
    pub shrink_order: u8,
    pub list: Arc<[Item]>,
}

// ---------------------------------------------------------------- state

#[derive(Clone, Debug, PartialEq)]
struct Col {
    u: Toks,
    /// Ends with `\endtemplate`.
    v: Toks,
    /// The tabskip glue after this column.
    glue: Glue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Term {
    None,
    Tab,
    Span,
    Cr,
}

#[derive(Clone, Debug, PartialEq)]
struct AlignState {
    valign: bool,
    display: bool,
    spec: Spec,
    first: Glue,
    cols: Vec<Col>,
    /// `&&`: the next column the periodic part copies.
    loop_col: Option<usize>,
    cur: usize,
    span_start: usize,
    omit: bool,
    term: Term,
    in_cell: bool,
    /// `\vadjust` material of the row's cells (placed after the row).
    adjust: Vec<Item>,
}

impl Datum for AlignState {
    fn same(&self, other: &dyn Datum) -> bool {
        other.as_any().downcast_ref::<AlignState>().is_some_and(|o| o == self)
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn level_key() -> Key {
    Key::Other(tag::ALIGN, 0)
}

fn load(c: &mut dyn Ctx) -> Option<(u32, AlignState)> {
    let l = c.int(&level_key());
    if l <= 0 {
        return None;
    }
    let l = l as u32;
    match c.get(&Key::Other(tag::ALIGN, l)) {
        Val::Data(d) => d.as_any().downcast_ref::<AlignState>().cloned().map(|s| (l, s)),
        _ => None,
    }
}

fn save(c: &mut dyn Ctx, l: u32, st: AlignState) {
    c.define(Key::Other(tag::ALIGN, l), Val::Data(Arc::new(st)), true);
}

// ---------------------------------------------------------------- tokens

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cls {
    Tab,
    Span,
    Cr,
    CrCr,
    Param,
    Spacer,
    LBrace,
    RBrace,
    MathShift,
    Omit,
    NoAlign,
    EndTemplate,
    Tabskip,
    Other,
}

fn by_cat(cat: u8) -> Cls {
    match cat {
        1 => Cls::LBrace,
        2 => Cls::RBrace,
        3 => Cls::MathShift,
        4 => Cls::Tab,
        6 => Cls::Param,
        10 => Cls::Spacer,
        _ => Cls::Other,
    }
}

fn classify(c: &mut dyn Ctx, t: &Tok) -> Cls {
    match t {
        Tok::Char { cat, .. } => by_cat(*cat),
        Tok::Param(_) => Cls::Param,
        Tok::DontExpand => Cls::Other,
        Tok::Cs(_) | Tok::Active(_) => match c.meaning(t) {
            Meaning::Char { cat, .. } => by_cat(cat),
            Meaning::Prim(id) => match registry().get(id).name {
                "span" => Cls::Span,
                "cr" => Cls::Cr,
                "crcr" => Cls::CrCr,
                "omit" => Cls::Omit,
                "noalign" => Cls::NoAlign,
                ENDTEMPLATE => Cls::EndTemplate,
                "tabskip" => Cls::Tabskip,
                _ => Cls::Other,
            },
            _ => Cls::Other,
        },
    }
}

fn expandable(c: &mut dyn Ctx, t: &Tok) -> bool {
    match t {
        Tok::Cs(_) | Tok::Active(_) => match c.meaning(t) {
            Meaning::Macro(_) | Meaning::Undefined => true,
            Meaning::Prim(id) => matches!(registry().get(id).kind, Kind::Expand(_) | Kind::If(_)),
            _ => false,
        },
        _ => false,
    }
}

fn cs(c: &mut dyn Ctx, name: &str) -> Tok {
    Tok::Cs(c.names().intern(name))
}

/// `\endtemplate` as a token.
pub fn endtemplate_tok(c: &mut dyn Ctx) -> Tok {
    cs(c, ENDTEMPLATE)
}

/// Get the next non-blank non-call token (tex.web 406).
fn next_nonblank_x(c: &mut dyn Ctx) -> Option<Tok> {
    loop {
        let t = c.next_x()?;
        if classify(c, &t) != Cls::Spacer {
            return Some(t);
        }
    }
}

fn tabskip_key() -> Key {
    Key::GluePar(gp::of("tabskip"))
}

fn tab_sub() -> u8 {
    gp::of("tabskip") as u8 + 1
}

// ---------------------------------------------------------------- commands

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

fn align_cmd(c: &mut dyn Ctx, code: u16) {
    let valign = code == 1;
    let me = cs(c, if valign { "valign" } else { "halign" });
    let mode = c.mode();
    match (valign, mode) {
        (false, Mode::Vertical { .. }) => init_align(c, false, false),
        (false, Mode::Horizontal { inner: false }) => {
            // head_for_vmode
            c.back(me);
            let par = cs(c, "par");
            c.push(Arc::from(vec![par]), TokKind::Inserted);
        }
        (false, Mode::Horizontal { inner: true }) => crate::groups::off_save(c, me),
        (false, Mode::Math { display: true }) => {
            if c.group_kind() == GroupKind::MathShift {
                init_align(c, false, true);
            } else {
                crate::groups::off_save(c, me);
            }
        }
        (false, Mode::Math { display: false }) => {
            let msg = format!("You can't use `\\halign' in {}", mode_name(mode));
            crate::diag::error(
                c,
                &msg,
                &[
                    "Sorry, but I'm not programmed to handle this case;",
                    "I'll just pretend that you didn't ask for it.",
                    "If you're in the wrong mode, you might be able to",
                    "return to the right one by typing `I}' or `I$' or `I\\par'.",
                ],
            );
        }
        (true, Mode::Vertical { .. }) => {
            c.back(me);
            crate::list::new_graf(c, true);
        }
        (true, Mode::Horizontal { .. }) => init_align(c, true, false),
        (true, Mode::Math { .. }) => {
            // insert_dollar_sign
            c.back(me);
            c.push(Arc::from(vec![Tok::Char { cat: 3, ch: '$' }]), TokKind::Inserted);
            crate::diag::error(
                c,
                "Missing $ inserted",
                &["I've inserted a begin-math/end-math symbol since I think", "you left one out. Proceed, with fingers crossed."],
            );
        }
    }
}

fn car_ret_cmd(c: &mut dyn Ctx, code: u16) {
    let (term, name) = match code {
        0 => (Term::Span, "\\span"),
        1 => (Term::Cr, "\\cr"),
        _ => (Term::Cr, "\\crcr"),
    };
    cell_end(c, term, name, false);
}

/// A tab mark (category 4 character, or a name `\let` to one) reached as a
/// command. The main dispatcher calls this.
pub fn tab_mark(c: &mut dyn Ctx, t: Tok) {
    let ch = match t {
        Tok::Char { ch, .. } => ch,
        _ => match c.meaning(&t) {
            Meaning::Char { ch, .. } => ch,
            _ => '&',
        },
    };
    let name = format!("alignment tab character {ch}");
    cell_end(c, Term::Tab, &name, ch == '&');
}

/// `&` `\span` `\cr` at the cell's level: the v template replaces it (tex.web 789).
fn cell_end(c: &mut dyn Ctx, term: Term, name: &str, amp: bool) {
    if let Some((l, mut st)) = load(c) {
        let gk = c.group_kind();
        // brace-delimited groups hide the tab (TeX's align_state ≠ 0)
        let at_cell = matches!(gk, GroupKind::Align | GroupKind::SemiSimple | GroupKind::MathShift | GroupKind::MathLeft);
        if st.in_cell && st.term == Term::None && at_cell {
            st.term = term;
            let v = if st.omit {
                let e = endtemplate_tok(c);
                Arc::from(vec![e])
            } else {
                st.cols[st.cur].v.clone()
            };
            save(c, l, st);
            c.push(v, TokKind::VTemplate);
            return;
        }
    }
    align_error(c, name, amp);
}

fn align_error(c: &mut dyn Ctx, name: &str, amp: bool) {
    let msg = format!("Misplaced {name}");
    if amp {
        crate::diag::error(
            c,
            &msg,
            &[
                "I can't figure out why you would want to use a tab mark",
                "here. If you just want an ampersand, the remedy is",
                "simple: Just type `I\\&' now. But if some right brace",
                "up above has ended a previous alignment prematurely,",
                "you're probably due for more error messages, and you",
                "might try typing `S' now just to see what is salvageable.",
            ],
        );
    } else {
        crate::diag::error(
            c,
            &msg,
            &[
                "I can't figure out why you would want to use a tab mark",
                "or \\cr or \\span just now. If something like a right brace",
                "up above has ended a previous alignment prematurely,",
                "you're probably due for more error messages, and you",
                "might try typing `S' now just to see what is salvageable.",
            ],
        );
    }
}

fn omit_cmd(c: &mut dyn Ctx, _code: u16) {
    crate::diag::error(
        c,
        "Misplaced \\omit",
        &["I expect to see \\omit only after tab marks or the \\cr of", "an alignment. Proceed, and I'll ignore this case."],
    );
}

fn noalign_cmd(c: &mut dyn Ctx, _code: u16) {
    crate::diag::error(
        c,
        "Misplaced \\noalign",
        &["I expect to see \\noalign only after the \\cr of", "an alignment. Proceed, and I'll ignore this case."],
    );
}

/// `\endtemplate` (do_endv, tex.web 1131).
fn endv_cmd(c: &mut dyn Ctx, _code: u16) {
    if c.group_kind() != GroupKind::Align {
        let t = endtemplate_tok(c);
        crate::groups::off_save(c, t);
        return;
    }
    let Some((l, st)) = load(c) else {
        crate::diag::error(c, "This can't happen (endv)", &[]);
        return;
    };
    if c.mode() == (Mode::Horizontal { inner: false }) {
        crate::list::end_graf(c);
    }
    fin_col(c, l, st);
}

/// A group of an alignment met its `}` (called by `groups::right_brace`
/// with the group still open): `Align` → "Missing \cr inserted"; `NoAlign` →
/// close it and look at what follows (tex.web 1132–1133).
pub fn align_group_end(c: &mut dyn Ctx, kind: GroupKind) {
    match kind {
        GroupKind::NoAlign => {
            if c.mode() == (Mode::Horizontal { inner: false }) {
                crate::list::end_graf(c);
            }
            c.close_group();
            if let Some((l, st)) = load(c) {
                align_peek(c, l, st);
            }
        }
        _ => {
            c.back(Tok::Char { cat: 2, ch: '}' });
            let cr = cs(c, "cr");
            c.push(Arc::from(vec![cr]), TokKind::Inserted);
            crate::diag::error(c, "Missing \\cr inserted", &["I'm guessing that you meant to end an alignment here."]);
        }
    }
}

// ---------------------------------------------------------------- init_align

fn init_align(c: &mut dyn Ctx, valign: bool, display: bool) {
    if display && c.tail().is_some() {
        crate::diag::error(
            c,
            "Improper \\halign inside $$'s",
            &[
                "Displays can use special alignments (like \\eqalignno)",
                "only if nothing but the alignment itself is between $$'s.",
                "So I've deleted the formulas that preceded this alignment.",
            ],
        );
        crate::math::flush_math(c);
    }
    let aux_key = if valign { Key::SpaceFactor } else { Key::PrevDepth };
    let aux = c.int(&aux_key);
    c.open_list(if valign { Mode::Horizontal { inner: true } } else { Mode::Vertical { inner: true } });
    c.define(aux_key, Val::Int(aux), false);
    // scan_spec(align_group)
    let spec = if crate::scan::scan_keyword(c, "to") {
        Spec::Exactly(crate::scan::scan_normal_dimen(c))
    } else if crate::scan::scan_keyword(c, "spread") {
        Spec::Additional(crate::scan::scan_normal_dimen(c))
    } else {
        Spec::Additional(0)
    };
    c.open_group(GroupKind::Align);
    crate::scan::scan_left_brace(c);
    let l = c.int(&level_key()).max(0) + 1;
    c.define(level_key(), Val::Int(l), false);
    let l = l as u32;

    // the preamble (tex.web 777–784)
    let first = c.get(&tabskip_key()).glue();
    let mut cols: Vec<Col> = Vec::new();
    let mut loop_col = None;
    let mut depth = 0i32;
    'pre: loop {
        // u_j
        let mut u: Vec<Tok> = Vec::new();
        loop {
            let Some(t) = preamble_token(c) else { break 'pre };
            let k = classify(c, &t);
            if k == Cls::Param {
                break;
            }
            if matches!(k, Cls::Tab | Cls::Cr | Cls::CrCr) && depth == 0 {
                if u.is_empty() && loop_col.is_none() && k == Cls::Tab {
                    loop_col = Some(cols.len());
                    continue;
                }
                c.back(t);
                crate::diag::error(
                    c,
                    "Missing # inserted in alignment preamble",
                    &[
                        "There should be exactly one # between &'s, when an",
                        "\\halign or \\valign is being set up. In this case you had",
                        "none, so I've put one in; maybe that will work.",
                    ],
                );
                break;
            }
            if k == Cls::Spacer && u.is_empty() {
                continue;
            }
            track(&t, &mut depth);
            u.push(t);
        }
        // v_j
        let mut v: Vec<Tok> = Vec::new();
        let end;
        loop {
            let Some(t) = preamble_token(c) else { break 'pre };
            let k = classify(c, &t);
            if matches!(k, Cls::Tab | Cls::Cr | Cls::CrCr) && depth == 0 {
                end = k;
                break;
            }
            if k == Cls::Param {
                crate::diag::error(
                    c,
                    "Only one # is allowed per tab",
                    &[
                        "There should be exactly one # between &'s, when an",
                        "\\halign or \\valign is being set up. In this case you had",
                        "more than one, so I'm ignoring all but the first.",
                    ],
                );
                continue;
            }
            track(&t, &mut depth);
            v.push(t);
        }
        v.push(endtemplate_tok(c));
        let glue = c.get(&tabskip_key()).glue();
        cols.push(Col { u: Arc::from(u), v: Arc::from(v), glue });
        if end != Cls::Tab {
            break;
        }
    }
    if cols.is_empty() {
        // the input ended inside the preamble; keep the state sound
        let e = endtemplate_tok(c);
        cols.push(Col { u: Arc::from(Vec::new()), v: Arc::from(vec![e]), glue: first });
    }
    c.open_group(GroupKind::Align);
    let st = AlignState {
        valign,
        display,
        spec,
        first,
        cols,
        loop_col,
        cur: 0,
        span_start: 0,
        omit: false,
        term: Term::None,
        in_cell: false,
        adjust: Vec::new(),
    };
    push_every_cr(c);
    align_peek(c, l, st);
}

fn track(t: &Tok, depth: &mut i32) {
    match t {
        Tok::Char { cat: 1, .. } => *depth += 1,
        Tok::Char { cat: 2, .. } => *depth -= 1,
        _ => {}
    }
}

/// get_preamble_token (tex.web 782): `\span` expands the next token once;
/// `\tabskip` assignments are performed and skipped.
fn preamble_token(c: &mut dyn Ctx) -> Option<Tok> {
    loop {
        let mut t = c.next()?;
        while classify(c, &t) == Cls::Span {
            t = c.next()?;
            if expandable(c, &t) {
                crate::expand::expand(c, t);
                t = c.next()?;
            }
        }
        match classify(c, &t) {
            Cls::EndTemplate => {
                crate::diag::error(c, "(interwoven alignment preambles are not allowed)", &[]);
                return Some(t);
            }
            Cls::Tabskip => {
                crate::scan::scan_optional_equals(c);
                let g = crate::scan::scan_glue(c, crate::prim::Level::Glue);
                let global = c.int(&Key::IntPar(ip::of("globaldefs"))) > 0;
                c.define(tabskip_key(), Val::Glue(g), global);
            }
            _ => return Some(t),
        }
    }
}

fn push_every_cr(c: &mut dyn Ctx) {
    let t = c.get(&Key::TokPar(tp::of("everycr"))).toks();
    if !t.is_empty() {
        c.push(t, TokKind::EveryCr);
    }
}

// ---------------------------------------------------------------- rows and cells

/// align_peek (tex.web 785).
fn align_peek(c: &mut dyn Ctx, l: u32, mut st: AlignState) {
    loop {
        let Some(t) = next_nonblank_x(c) else {
            save(c, l, st);
            return;
        };
        match classify(c, &t) {
            Cls::NoAlign => {
                crate::scan::scan_left_brace(c);
                c.open_group(GroupKind::NoAlign);
                if !st.valign {
                    crate::list::normal_paragraph(c);
                }
                st.in_cell = false;
                save(c, l, st);
                return;
            }
            Cls::RBrace => {
                fin_align(c, st);
                return;
            }
            Cls::CrCr => continue,
            _ => {
                init_row(c, &mut st);
                init_col(c, &mut st, t);
                save(c, l, st);
                return;
            }
        }
    }
}

fn row_mode(st: &AlignState) -> Mode {
    if st.valign { Mode::Vertical { inner: true } } else { Mode::Horizontal { inner: true } }
}

fn init_row(c: &mut dyn Ctx, st: &mut AlignState) {
    c.open_list(row_mode(st));
    if st.valign {
        c.define(Key::PrevDepth, Val::Int(0), false);
    } else {
        c.define(Key::SpaceFactor, Val::Int(0), false);
    }
    c.item(Item::Glue { g: st.first, sub: tab_sub(), leader: None });
    st.cur = 0;
    st.adjust.clear();
    init_span(c, st, 0);
}

fn init_span(c: &mut dyn Ctx, st: &mut AlignState, col: usize) {
    c.open_list(row_mode(st));
    if st.valign {
        c.define(Key::PrevDepth, Val::Int(IGNORE_DEPTH), false);
        crate::list::normal_paragraph(c);
    } else {
        c.define(Key::SpaceFactor, Val::Int(1000), false);
    }
    st.span_start = col;
}

fn init_col(c: &mut dyn Ctx, st: &mut AlignState, t: Tok) {
    st.term = Term::None;
    st.in_cell = true;
    if classify(c, &t) == Cls::Omit {
        st.omit = true;
    } else {
        st.omit = false;
        c.back(t);
        c.push(st.cols[st.cur].u.clone(), TokKind::UTemplate);
    }
}

fn totals(list: &[Item]) -> ([i32; 4], [i32; 4]) {
    let mut s = [0i32; 4];
    let mut h = [0i32; 4];
    for it in list {
        if let Item::Glue { g, .. } = it {
            let so = g.stretch_order.min(3) as usize;
            let ho = g.shrink_order.min(3) as usize;
            s[so] = s[so].wrapping_add(g.stretch);
            h[ho] = h[ho].wrapping_add(g.shrink);
        }
    }
    (s, h)
}

fn top_order(t: &[i32; 4]) -> u8 {
    (1..4).rev().find(|&o| t[o] != 0).unwrap_or(0) as u8
}

/// fin_col (tex.web 791); continues into fin_row at a `\cr`.
fn fin_col(c: &mut dyn Ctx, l: u32, mut st: AlignState) {
    // If the preamble list has been traversed, check that the row has ended (792–793).
    if st.cur + 1 >= st.cols.len() && st.term < Term::Cr {
        if let Some(lc) = st.loop_col {
            let src = st.cols[lc.min(st.cols.len() - 1)].clone();
            st.cols.push(src);
            st.loop_col = Some(lc + 1);
        } else {
            crate::diag::error(
                c,
                "Extra alignment tab has been changed to \\cr",
                &[
                    "You have given more \\span or & marks than there were",
                    "in the preamble to the \\halign or \\valign now in progress.",
                    "So I'll assume that you meant to type \\cr instead.",
                ],
            );
            st.term = Term::Cr;
        }
    }
    if st.term != Term::Span {
        c.close_group();
        c.open_group(GroupKind::Align);
        // Package an unset box for the current column (796).
        let items = c.close_list();
        let (b, adjust) = if st.valign {
            let p = vpack(items, Spec::Additional(0), 0, &Params::default());
            (p.b, p.adjust)
        } else {
            let p = hpack(items, Spec::Additional(0), &Params::default(), crate::font::fonts());
            (p.b, p.adjust)
        };
        st.adjust.extend(adjust);
        let (s, h) = totals(&b.list);
        let so = top_order(&s);
        let ho = top_order(&h);
        let n = (st.cur - st.span_start.min(st.cur)) as u16;
        let u = Unset {
            vertical: st.valign,
            width: b.width,
            height: b.height,
            depth: b.depth,
            span: n,
            stretch: s[so as usize],
            stretch_order: so,
            shrink: h[ho as usize],
            shrink_order: ho,
            list: b.list.clone(),
        };
        c.item(Item::Unset(Arc::new(u)));
        // Copy the tabskip glue between columns (795).
        c.item(Item::Glue { g: st.cols[st.cur].glue, sub: tab_sub(), leader: None });
        if st.term >= Term::Cr {
            st.in_cell = false;
            st.term = Term::None;
            st.omit = false;
            fin_row(c, l, st);
            return;
        }
        let next = st.cur + 1;
        init_span(c, &mut st, next);
    }
    st.in_cell = false;
    st.cur += 1;
    if let Some(t) = next_nonblank_x(c) {
        init_col(c, &mut st, t);
    }
    save(c, l, st);
}

/// append_to_vlist for an unset row (tex.web 679).
fn append_row(c: &mut dyn Ctx, row: Unset) {
    let pd = c.int(&Key::PrevDepth);
    if pd > IGNORE_DEPTH {
        let bs = c.get(&Key::GluePar(gp::of("baselineskip"))).glue();
        let d = bs.width - pd - row.height;
        let lsl = c.int(&Key::DimenPar(dp::of("lineskiplimit")));
        if d < lsl {
            let ls = c.get(&Key::GluePar(gp::of("lineskip"))).glue();
            c.item(Item::Glue { g: ls, sub: gp::of("lineskip") as u8 + 1, leader: None });
        } else {
            c.item(Item::Glue { g: Glue { width: d, ..bs }, sub: gp::of("baselineskip") as u8 + 1, leader: None });
        }
    }
    let depth = row.depth;
    c.item(Item::Unset(Arc::new(row)));
    c.define(Key::PrevDepth, Val::Int(depth), false);
}

/// fin_row (tex.web 799).
fn fin_row(c: &mut dyn Ctx, l: u32, mut st: AlignState) {
    let items = c.close_list();
    if st.valign {
        let p = vpack(items, Spec::Additional(0), MAX_DIMEN, &Params::default());
        let row = Unset {
            vertical: true,
            width: p.b.width,
            height: p.b.height,
            depth: p.b.depth,
            span: 0,
            stretch: 0,
            stretch_order: 0,
            shrink: 0,
            shrink_order: 0,
            list: p.b.list.clone(),
        };
        c.item(Item::Unset(Arc::new(row)));
        c.define(Key::SpaceFactor, Val::Int(1000), false);
    } else {
        let p = hpack(items, Spec::Additional(0), &Params::default(), crate::font::fonts());
        let row = Unset {
            vertical: false,
            width: p.b.width,
            height: p.b.height,
            depth: p.b.depth,
            span: 0,
            stretch: 0,
            stretch_order: 0,
            shrink: 0,
            shrink_order: 0,
            list: p.b.list.clone(),
        };
        append_row(c, row);
        for it in std::mem::take(&mut st.adjust) {
            c.item(it);
        }
    }
    st.adjust.clear();
    push_every_cr(c);
    align_peek(c, l, st);
}

// ---------------------------------------------------------------- fin_align

/// tex.web 810/811: the glue of a cell of natural size `nat` set to `t`.
fn glue_set(nat: i32, t: i32, u: &Unset) -> GlueSet {
    if t == nat {
        GlueSet::default()
    } else if t > nat {
        let ratio = if u.stretch == 0 { 0.0 } else { f64::from(t - nat) / f64::from(u.stretch) };
        GlueSet { sign: 1, order: u.stretch_order, ratio }
    } else {
        let ratio = if u.shrink == 0 {
            0.0
        } else if u.shrink_order == 0 && nat - t > u.shrink {
            1.0
        } else {
            f64::from(nat - t) / f64::from(u.shrink)
        };
        GlueSet { sign: -1, order: u.shrink_order, ratio }
    }
}

/// fin_align (tex.web 800–812).
fn fin_align(c: &mut dyn Ctx, st: AlignState) {
    c.close_group(); // the entries' group
    c.close_group(); // the whole alignment's group
    let valign = st.valign;
    let o = if st.display { c.int(&Key::DimenPar(dp::of("displayindent"))) } else { 0 };
    let aux_key = if valign { Key::SpaceFactor } else { Key::PrevDepth };
    let aux = c.int(&aux_key);
    let items = c.close_list();

    // Column widths (801–803): singles by max, spans folded left to right.
    let ncols = st.cols.len();
    let mut w = vec![NULL_FLAG; ncols];
    let mut spans: Vec<std::collections::BTreeMap<usize, i32>> = vec![Default::default(); ncols];
    for it in &items {
        if let Item::Unset(row) = it {
            let mut j = 0usize;
            for x in row.list.iter() {
                if let Item::Unset(cell) = x {
                    let n = cell.span as usize;
                    let cw = if valign { cell.height } else { cell.width };
                    if n == 0 {
                        if j < ncols {
                            w[j] = w[j].max(cw);
                        }
                    } else if j + n < ncols {
                        let e = spans[j].entry(j + n).or_insert(NULL_FLAG);
                        *e = (*e).max(cw);
                    }
                    j += n + 1;
                }
            }
        }
    }
    let mut glues: Vec<Glue> = st.cols.iter().map(|k| k.glue).collect();
    for j in 0..ncols {
        if w[j] == NULL_FLAG {
            w[j] = 0;
            glues[j] = Glue::default();
        }
        if !spans[j].is_empty() {
            let t = w[j] + glues[j].width;
            let sp = std::mem::take(&mut spans[j]);
            for (end, wd) in sp {
                let wd = wd - t;
                if end == j + 1 {
                    w[j + 1] = w[j + 1].max(wd);
                } else {
                    let e = spans[j + 1].entry(end).or_insert(NULL_FLAG);
                    *e = (*e).max(wd);
                }
            }
        }
    }

    // The prototype row (804).
    let sub = tab_sub();
    let mut pre: Vec<Item> = vec![Item::Glue { g: st.first, sub, leader: None }];
    for j in 0..ncols {
        let b = if valign {
            BoxNode { vertical: true, height: w[j], ..Default::default() }
        } else {
            BoxNode { width: w[j], ..Default::default() }
        };
        pre.push(Item::Box(Arc::new(b)));
        pre.push(Item::Glue { g: glues[j], sub, leader: None });
    }
    let packed = if valign {
        let pr = Params {
            vbadness: c.int(&Key::IntPar(ip::of("vbadness"))),
            vfuzz: c.int(&Key::DimenPar(dp::of("vfuzz"))),
            ..Default::default()
        };
        vpack(pre, st.spec, MAX_DIMEN, &pr)
    } else {
        let pr = Params {
            hbadness: c.int(&Key::IntPar(ip::of("hbadness"))),
            hfuzz: c.int(&Key::DimenPar(dp::of("hfuzz"))),
            overfull_rule: 0,
            ..Default::default()
        };
        hpack(pre, st.spec, &pr, crate::font::fonts())
    };
    if let Some(r) = packed.report.clone() {
        c.emit(Emit::Diag(r));
    }
    let p = packed.b;
    let lr = if st.display && !valign && c.etex() { 2 } else { 0 };

    // Set the glue in all the unset boxes (805–811).
    let mut out: Vec<Item> = Vec::with_capacity(items.len());
    for it in items {
        match it {
            Item::Unset(row) => {
                let mut list: Vec<Item> = Vec::with_capacity(row.list.len());
                let mut j = 0usize;
                for x in row.list.iter() {
                    let Item::Unset(cell) = x else {
                        list.push(x.clone());
                        continue;
                    };
                    let wj = w.get(j).copied().unwrap_or(0);
                    let mut t = wj;
                    let mut extra = Vec::new();
                    for _ in 0..cell.span {
                        let g = glues.get(j).copied().unwrap_or_default();
                        extra.push(Item::Glue { g, sub, leader: None });
                        t += g.width;
                        if p.set.sign == 1 {
                            if g.stretch_order == p.set.order {
                                t += (p.set.ratio * f64::from(g.stretch)).round() as i32;
                            }
                        } else if p.set.sign == -1 && g.shrink_order == p.set.order {
                            t -= (p.set.ratio * f64::from(g.shrink)).round() as i32;
                        }
                        j += 1;
                        let ws = w.get(j).copied().unwrap_or(0);
                        let nb = if valign {
                            BoxNode { vertical: true, height: ws, ..Default::default() }
                        } else {
                            BoxNode { width: ws, ..Default::default() }
                        };
                        extra.push(Item::Box(Arc::new(nb)));
                        t += ws;
                    }
                    let b = if valign {
                        BoxNode {
                            vertical: true,
                            width: row.width,
                            height: wj,
                            depth: cell.depth,
                            shift: 0,
                            set: glue_set(cell.height, t, cell),
                            lr: 0,
                            list: cell.list.clone(),
                        }
                    } else {
                        BoxNode {
                            vertical: false,
                            width: wj,
                            height: row.height,
                            depth: row.depth,
                            shift: 0,
                            set: glue_set(cell.width, t, cell),
                            lr: 0,
                            list: cell.list.clone(),
                        }
                    };
                    list.push(Item::Box(Arc::new(b)));
                    list.extend(extra);
                    j += 1;
                }
                let b = if valign {
                    BoxNode {
                        vertical: true,
                        width: row.width,
                        height: p.height,
                        depth: row.depth,
                        shift: o,
                        set: p.set,
                        lr: 0,
                        list: Arc::from(list),
                    }
                } else {
                    BoxNode {
                        vertical: false,
                        width: p.width,
                        height: row.height,
                        depth: row.depth,
                        shift: o,
                        set: p.set,
                        lr,
                        list: Arc::from(list),
                    }
                };
                out.push(Item::Box(Arc::new(b)));
            }
            Item::Rule { w: rw, h, d } => {
                // running dimensions extend to the alignment's boundaries (806)
                let rw = if rw == RUNNING { p.width } else { rw };
                let h = if h == RUNNING { p.height } else { h };
                let d = if d == RUNNING { p.depth } else { d };
                let r = Item::Rule { w: rw, h, d };
                if o != 0 {
                    let b = BoxNode {
                        vertical: false,
                        width: rw,
                        height: h,
                        depth: d,
                        shift: o,
                        set: GlueSet::default(),
                        lr: 0,
                        list: Arc::from(vec![r]),
                    };
                    out.push(Item::Box(Arc::new(b)));
                } else {
                    out.push(r);
                }
            }
            other => out.push(other),
        }
    }

    // Insert the current list into its environment (812).
    if st.display {
        finish_display(c, out, aux);
    } else {
        for it in out {
            c.item(it);
        }
        c.define(aux_key, Val::Int(aux), false);
    }
}

fn is_math_shift(c: &mut dyn Ctx, t: &Option<Tok>) -> bool {
    match t {
        Some(t) => classify(c, t) == Cls::MathShift,
        None => false,
    }
}

/// Finish an alignment in a display (tex.web 1206–1207).
fn finish_display(c: &mut dyn Ctx, items: Vec<Item>, aux: i32) {
    let t = crate::assign::do_assignments(c);
    if !is_math_shift(c, &t) {
        if let Some(t) = t {
            c.back(t);
        }
        crate::diag::error(
            c,
            "Missing $$ inserted",
            &["Displays can use special alignments (like \\eqalignno)", "only if nothing but the alignment itself is between $$'s."],
        );
    } else {
        let t2 = c.next_x();
        if !is_math_shift(c, &t2) {
            if let Some(t2) = t2 {
                c.back(t2);
            }
            crate::diag::error(
                c,
                "Display math should end with $$",
                &["The `$' that I just saw supposedly matches a previous `$$'.", "So I shall assume that you typed `$$' both times."],
            );
        }
    }
    let _ = c.close_list(); // the display's math list
    let pre = c.int(&Key::IntPar(ip::of("predisplaypenalty")));
    c.item(Item::Penalty(pre));
    let above = c.get(&Key::GluePar(gp::of("abovedisplayskip"))).glue();
    c.item(Item::Glue { g: above, sub: gp::of("abovedisplayskip") as u8 + 1, leader: None });
    for it in items {
        c.item(it);
    }
    let post = c.int(&Key::IntPar(ip::of("postdisplaypenalty")));
    c.item(Item::Penalty(post));
    let below = c.get(&Key::GluePar(gp::of("belowdisplayskip"))).glue();
    c.item(Item::Glue { g: below, sub: gp::of("belowdisplayskip") as u8 + 1, leader: None });
    c.define(Key::PrevDepth, Val::Int(aux), false);
    crate::math::resume_after_display(c);
}
