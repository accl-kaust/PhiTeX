//! C1: building lists: characters (ligatures/kerns from the font), spaces and
//! space factor, `\char`, `\accent`, glue/kern/penalty/rule commands, `\-`
//! `\discretionary`, `\indent` `\noindent` `\par`, `\vadjust` `\insert` `\mark`,
//! `\unskip` & co, `\end`. Paragraph end calls `linebreak::line_break`.
//!
//! tex.web parts 45–47 as handlers. Every handler is a node: it scans its
//! arguments through `scan.rs`, reads through `Ctx`, and contributes through
//! `emit`/`define`. Nothing is kept between nodes but what is in env:
//! * the open `\discretionary`'s part and finished lists: `Key::Other(LIST, DISC)`,
//!   local to its `Disc` group;
//! * the open `\insert`'s class (255 `\vadjust`): `Key::Other(LIST, INSERT)`,
//!   local to its `Insert` group;
//! * a paragraph list's initial language (`(lhm*64+rhm)*65536+lang`, TeX's
//!   paragraph `pg_field`) in that list's `Key::PrevDepth` (unused in
//!   horizontal mode), and a mirror of the enclosing vertical list's
//!   `\prevgraf` in its `Key::PrevGraf`, copied back when it ends.
//!
//! Character tokens (cat 10, 11, 12 and tokens meaning them or a `\chardef`
//! char) are dispatched by the reducer to `char_tok`.
use std::sync::Arc;

use crate::ctx::Ctx;
use crate::diag::{error, Diag};
use crate::env::{GroupKind, Mode};
use crate::font::{fonts, LigKern};
use crate::item::{BoxNode, Disc, Ins, Item, Whatsit};
use crate::prim::{dp, gp, ip, registry, tag, Kind, Level, Registry};
use crate::scan;
use crate::term::{Emit, TokKind};
use crate::tok::Tok;
use crate::val::{Datum, FontId, Glue, Key, Meaning, Val};

pub const IGNORE_DEPTH: i32 = -65_536_000;
pub const MAX_DIMEN: i32 = 0o7777777777;
const DEFAULT_RULE: i32 = 26214;
const RUNNING: i32 = crate::item::RUNNING;

/// `Key::Other(tag::LIST, n)` numbers.
pub mod other {
    /// The open `\discretionary` (`Val::Data(DiscState)`).
    pub const DISC: u32 = 0;
    /// The open `\insert`/`\vadjust`'s class (`Val::Int`, 255 `\vadjust`).
    pub const INSERT: u32 = 1;
}

// ---------------------------------------------------------------- registry

pub fn register(r: &mut Registry) {
    r.add("char", 0, Kind::Command(char_num_cmd), 0);
    r.add("noboundary", 0, Kind::Command(no_boundary_cmd), 0);
    r.add(" ", 0, Kind::Command(ex_space_cmd), 0);
    r.add("/", 0, Kind::Command(ital_corr_cmd), 0);
    r.add("accent", 0, Kind::Command(accent_cmd), 0);
    for (n, code) in [("hfil", 0u16), ("hfill", 1), ("hss", 2), ("hfilneg", 3), ("hskip", 4)] {
        r.add(n, code, Kind::Command(hskip_cmd), 0);
    }
    for (n, code) in [("vfil", 0u16), ("vfill", 1), ("vss", 2), ("vfilneg", 3), ("vskip", 4)] {
        r.add(n, code, Kind::Command(vskip_cmd), 0);
    }
    r.add("kern", 0, Kind::Command(kern_cmd), 0);
    r.add("penalty", 0, Kind::Command(penalty_cmd), 0);
    r.add("hrule", 0, Kind::Command(hrule_cmd), 0);
    r.add("vrule", 0, Kind::Command(vrule_cmd), 0);
    r.add("unpenalty", 0, Kind::Command(remove_item_cmd), 0);
    r.add("unkern", 1, Kind::Command(remove_item_cmd), 0);
    r.add("unskip", 2, Kind::Command(remove_item_cmd), 0);
    r.add("discretionary", 0, Kind::Command(disc_cmd), 0);
    r.add("-", 1, Kind::Command(disc_cmd), 0);
    r.add("noindent", 0, Kind::Command(start_par_cmd), 0);
    r.add("indent", 1, Kind::Command(start_par_cmd), 0);
    r.add("par", 0, Kind::Command(par_cmd), 0);
    r.add("insert", 0, Kind::Command(insert_cmd), 0);
    r.add("vadjust", 1, Kind::Command(insert_cmd), 0);
    r.add("mark", 0, Kind::Command(mark_cmd), 0);
    r.add("marks", 1, Kind::Command(mark_cmd), 1);
    r.add("end", 0, Kind::Command(stop_cmd), 0);
    r.add("dump", 1, Kind::Command(stop_cmd), 0);
    r.add("lastpenalty", 0, Kind::Computed { level: Level::Int, get: last_item_get, set: None }, 0);
    r.add("lastkern", 1, Kind::Computed { level: Level::Dimen, get: last_item_get, set: None }, 0);
    r.add("lastskip", 2, Kind::Computed { level: Level::Glue, get: last_item_get, set: None }, 0);
    r.add("lastnodetype", 3, Kind::Computed { level: Level::Int, get: last_item_get, set: None }, 1);
    r.add("spacefactor", 0, Kind::Computed { level: Level::Int, get: aux_get, set: Some(aux_set) }, 0);
    r.add("prevdepth", 1, Kind::Computed { level: Level::Dimen, get: aux_get, set: Some(aux_set) }, 0);
    r.add("prevgraf", 0, Kind::Computed { level: Level::Int, get: prev_graf_get, set: Some(prev_graf_set) }, 0);
}

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

fn esc(c: &mut dyn Ctx, name: &str) -> String {
    let e = c.int(&Key::IntPar(ip::of("escapechar")));
    match char::from_u32(e as u32) {
        Some(ch) if (0..256).contains(&e) => format!("{ch}{name}"),
        _ => name.to_string(),
    }
}

fn you_cant(c: &mut dyn Ctx, name: &str, help: &[&str]) {
    let m = c.mode();
    let n = esc(c, name);
    let msg = format!("You can't use `{n}' in {}", mode_name(m));
    error(c, &msg, help);
}

/// `report_illegal_case`.
pub fn report_illegal_case(c: &mut dyn Ctx, name: &str) {
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

/// A horizontal command in vertical mode: `back_input; new_graf(true)`.
pub fn back_into_paragraph(c: &mut dyn Ctx, t: Tok) {
    c.back(t);
    new_graf(c, true);
}

/// `head_for_vmode` (tex.web 1095): a vertical command in horizontal mode.
pub fn head_for_vmode(c: &mut dyn Ctx, t: Tok, hrule: bool) {
    if let Mode::Horizontal { inner: true } = c.mode() {
        if !hrule {
            crate::groups::off_save(c, t);
        } else {
            let n = esc(c, "hrule");
            error(
                c,
                &format!("You can't use `{n}' here except with leaders"),
                &["To put a horizontal rule in an hbox or an alignment,", "you should use \\leaders or \\hrulefill (see The TeXbook)."],
            );
        }
    } else {
        c.back(t);
        let par = Tok::Cs(c.names().intern("par"));
        c.push(Arc::from(vec![par]), TokKind::Inserted);
    }
}

/// `insert_dollar_sign` (tex.web 1047).
pub fn insert_dollar_sign(c: &mut dyn Ctx, t: Tok) {
    c.back(t);
    c.push(Arc::from(vec![Tok::Char { cat: 3, ch: '$' }]), TokKind::Inserted);
    error(
        c,
        "Missing $ inserted",
        &["I've inserted a begin-math/end-math symbol since I think", "you left one out. Proceed, with fingers crossed."],
    );
}

/// For a horizontal command: true when it may act here; in vertical mode the
/// paragraph is started (the command is read again after `\everypar`).
fn horizontal_here(c: &mut dyn Ctx, name: &str) -> bool {
    if is_v(c.mode()) {
        let t = self_tok(c, name);
        back_into_paragraph(c, t);
        return false;
    }
    true
}

/// For a vertical command: true when it may act here.
fn vertical_here(c: &mut dyn Ctx, name: &str, hrule: bool) -> bool {
    let m = c.mode();
    if is_h(m) {
        let t = self_tok(c, name);
        head_for_vmode(c, t, hrule);
        return false;
    }
    if is_m(m) {
        let t = self_tok(c, name);
        insert_dollar_sign(c, t);
        return false;
    }
    true
}

fn ipar(c: &mut dyn Ctx, n: &str) -> i32 {
    c.int(&Key::IntPar(ip::of(n)))
}
fn dpar(c: &mut dyn Ctx, n: &str) -> i32 {
    c.int(&Key::DimenPar(dp::of(n)))
}
fn gpar(c: &mut dyn Ctx, n: &str) -> Glue {
    c.get(&Key::GluePar(gp::of(n))).glue()
}

fn cur_font(c: &mut dyn Ctx) -> FontId {
    match c.get(&Key::CurFont) {
        Val::Font(f) => f,
        Val::Int(n) => n as FontId,
        _ => 0,
    }
}

fn fparam(c: &mut dyn Ctx, f: FontId, n: u16) -> i32 {
    crate::font::font_dimen(c, f, n)
}

fn key_char(ch: u16) -> char {
    char::from_u32(ch as u32).unwrap_or('\0')
}

fn norm_min(h: i32) -> i32 {
    h.clamp(1, 63)
}

fn cur_lang(c: &mut dyn Ctx) -> i32 {
    let l = ipar(c, "language");
    if !(1..=255).contains(&l) { 0 } else { l }
}

/// tex.web `xn_over_d` (truncating, sign of x).
fn xn_over_d(x: i32, n: i32, d: i32) -> i32 {
    if d == 0 {
        return 0;
    }
    let q = (x.unsigned_abs() as i64 * n as i64) / d as i64;
    let q = q.clamp(0, i32::MAX as i64) as i32;
    if x < 0 { -q } else { q }
}

fn glue_item(g: Glue, sub: u8) -> Item {
    Item::Glue { g, sub, leader: None }
}

/// `new_param_glue(n)`: subtype n+1.
fn param_glue(c: &mut dyn Ctx, n: &str) -> Item {
    let i = gp::of(n);
    let g = c.get(&Key::GluePar(i)).glue();
    glue_item(g, i as u8 + 1)
}

fn null_box(width: i32) -> Item {
    Item::Box(Arc::new(BoxNode { width, ..Default::default() }))
}

fn set_space_factor(c: &mut dyn Ctx, v: i32) {
    c.define(Key::SpaceFactor, Val::Int(v), false);
}

/// `char_warning`.
fn char_warning(c: &mut dyn Ctx, f: FontId, ch: u16) {
    if ipar(c, "tracinglostchars") > 0 {
        let name = fonts().font(f).name().to_string();
        let shown = match char::from_u32(ch as u32) {
            Some(x) if (32..127).contains(&ch) => x.to_string(),
            _ => format!("^^{:02x}", ch),
        };
        let line = c.line_no();
        let msg = format!("Missing character: There is no {shown} in font {name}!");
        c.emit(Emit::Diag(Arc::new(Diag { msg, help: Vec::new(), line })));
    }
}

/// `new_character`.
fn new_character(c: &mut dyn Ctx, f: FontId, ch: u16) -> Option<Item> {
    if fonts().font(f).has(ch) {
        Some(Item::Char { font: f, c: ch })
    } else {
        char_warning(c, f, ch);
        None
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cls {
    /// letter, other_char, char_given
    Ch(u16),
    CharNum,
    NoBoundary,
    Space,
    Other,
}

fn classify(c: &mut dyn Ctx, t: &Tok) -> Cls {
    match t {
        Tok::Char { cat: 11 | 12, ch } => Cls::Ch(*ch as u32 as u16),
        Tok::Char { cat: 10, .. } => Cls::Space,
        Tok::Cs(_) | Tok::Active(_) => match c.meaning(t) {
            Meaning::Char { cat: 11 | 12, ch } => Cls::Ch(ch as u32 as u16),
            Meaning::Char { cat: 10, .. } => Cls::Space,
            Meaning::CharGiven(n) => Cls::Ch(n as u16),
            Meaning::Prim(id) => match registry().get(id).name {
                "char" => Cls::CharNum,
                "noboundary" => Cls::NoBoundary,
                _ => Cls::Other,
            },
            _ => Cls::Other,
        },
        _ => Cls::Other,
    }
}

fn adjust_space_factor(c: &mut dyn Ctx, sf: &mut i32, ch: u16) {
    let s = c.int(&Key::Sf(key_char(ch)));
    if s == 1000 {
        *sf = 1000;
    } else if s < 1000 {
        if s > 0 {
            *sf = s;
        }
    } else if *sf < 1000 {
        *sf = 1000;
    } else {
        *sf = s;
    }
}

// ---------------------------------------------------------------- characters

/// A character token (cat 10/11/12, or a token meaning one or a `\chardef`
/// char) in command position. `t` is the token as read (put back in
/// vertical mode).
pub fn char_tok(c: &mut dyn Ctx, t: Tok) {
    match classify(c, &t) {
        Cls::Ch(ch) => char_like(c, t, ch),
        Cls::Space => space_tok(c),
        Cls::CharNum => char_num_cmd(c, 0),
        Cls::NoBoundary => no_boundary_cmd(c, 0),
        Cls::Other => {}
    }
}

/// A letter/other char/char-given `ch` met as `t`.
pub fn char_like(c: &mut dyn Ctx, t: Tok, ch: u16) {
    match c.mode() {
        Mode::Vertical { .. } => back_into_paragraph(c, t),
        Mode::Horizontal { .. } => main_loop(c, ch, false),
        Mode::Math { .. } => crate::math::letter(c, u32::from(ch)),
    }
}

fn char_num_cmd(c: &mut dyn Ctx, _code: u16) {
    match c.mode() {
        Mode::Vertical { .. } => {
            let t = self_tok(c, "char");
            back_into_paragraph(c, t);
        }
        Mode::Horizontal { .. } => {
            let ch = scan::scan_char_num(c);
            main_loop(c, ch, false);
        }
        Mode::Math { .. } => {
            let ch = scan::scan_char_num(c);
            crate::math::letter(c, u32::from(ch));
        }
    }
}

/// `\noboundary` (tex.web 1045): in horizontal mode a following character
/// starts without the left boundary.
fn no_boundary_cmd(c: &mut dyn Ctx, _code: u16) {
    match c.mode() {
        Mode::Vertical { .. } => {
            let t = self_tok(c, "noboundary");
            back_into_paragraph(c, t);
        }
        Mode::Horizontal { .. } => {
            let Some(t) = c.next_x() else { return };
            match classify(c, &t) {
                Cls::Ch(ch) => main_loop(c, ch, true),
                Cls::CharNum => {
                    let ch = scan::scan_char_num(c);
                    main_loop(c, ch, true);
                }
                _ => c.back(t),
            }
        }
        Mode::Math { .. } => {}
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum R {
    C(u16),
    /// The right boundary (`bchar`).
    Bound,
    /// `non_char`.
    No,
}

#[derive(Clone, Copy)]
enum Ent {
    Ch(u16),
    Lig { c: u16, ptr: Option<u16> },
}
impl Ent {
    fn ch(self) -> u16 {
        match self {
            Ent::Ch(c) | Ent::Lig { c, .. } => c,
        }
    }
}

#[derive(Clone, Copy)]
enum L {
    Wrapup,
    Move,
    Move1,
    Move2,
    Lookahead,
    LigLoop,
}

struct Ml {
    f: FontId,
    font: Arc<crate::font::Font>,
    no_ligs: bool,
    out: Vec<Item>,
    /// `cur_q`: the items from here on are the pending ligature's components.
    q: usize,
    cur_l: Option<u16>,
    cur_r: R,
    /// `lig_stack`, top last.
    stack: Vec<Ent>,
    lig_present: bool,
    lft_hit: bool,
    rt_hit: bool,
    ins_disc: bool,
    bchar_on: bool,
    hyphen: i32,
    outer_h: bool,
}

impl Ml {
    fn lookup(&self) -> LigKern {
        let lk = match (self.cur_l, self.cur_r) {
            (_, R::No) => LigKern::None,
            (Some(a), R::C(b)) => self.font.lig_kern(a, b),
            (Some(a), R::Bound) => self.font.lig_kern_boundary(Some(a), None),
            (None, R::C(b)) => self.font.lig_kern_boundary(None, Some(b)),
            (None, R::Bound) => self.font.lig_kern_boundary(None, None),
        };
        match lk {
            LigKern::Lig { .. } if self.no_ligs => LigKern::None,
            x => x,
        }
    }
    fn bchar(&self) -> R {
        if self.bchar_on && self.font.bchar().is_some() { R::Bound } else { R::No }
    }
    /// `pack_lig`.
    fn pack_lig(&mut self, rt: bool) {
        let Some(l) = self.cur_l else { return };
        let comps: Vec<u16> = self
            .out
            .drain(self.q..)
            .filter_map(|it| match it {
                Item::Char { c, .. } => Some(c),
                _ => None,
            })
            .collect();
        let left = self.lft_hit;
        self.lft_hit = false;
        let mut right = false;
        if rt && self.stack.is_empty() {
            right = true;
            self.rt_hit = false;
        }
        self.out.push(Item::Lig { font: self.f, c: l, orig: Arc::from(comps), left, right });
        self.lig_present = false;
    }
    /// `wrapup`.
    fn wrapup(&mut self, rt: bool) {
        if self.cur_l.is_none() {
            return;
        }
        if self.out.len() > self.q {
            if let Some(Item::Char { c: last, .. }) = self.out.last() {
                if *last as i32 == self.hyphen {
                    self.ins_disc = true;
                }
            }
        }
        if self.lig_present {
            self.pack_lig(rt);
        }
        if self.ins_disc {
            self.ins_disc = false;
            if self.outer_h {
                self.out.push(Item::Disc(Arc::new(Disc {
                    pre: Arc::from(Vec::new()),
                    post: Arc::from(Vec::new()),
                    replace: 0,
                    explicit: false,
                })));
            }
        }
    }
}

/// The main loop (tex.web 1034–1040): a run of characters with ligatures and
/// kerns from the font's program, boundary characters, space factor, and
/// an empty discretionary after a hyphen character. Ends on the first
/// token that is not a character, which is put back.
pub fn main_loop(c: &mut dyn Ctx, first: u16, cancel_boundary: bool) {
    let mut sf = c.int(&Key::SpaceFactor);
    adjust_space_factor(c, &mut sf, first);
    let f = cur_font(c);
    let mode = c.mode();
    let outer_h = mode == Mode::Horizontal { inner: false };
    if outer_h {
        // fix_language
        let l = cur_lang(c);
        if l != c.int(&Key::Clang) {
            let lhm = norm_min(ipar(c, "lefthyphenmin")) as u8;
            let rhm = norm_min(ipar(c, "righthyphenmin")) as u8;
            c.item(Item::Whatsit(Arc::new(Whatsit::Language { lang: l as u8, lhm, rhm })));
            c.define(Key::Clang, Val::Int(l), false);
        }
    }
    let hyphen = crate::font::hyphen_char(c, f);
    let no_ligs = c.pdf() && crate::font::no_ligatures(c, f);
    let mut m = Ml {
        f,
        font: fonts().font(f),
        no_ligs,
        out: Vec::new(),
        q: 0,
        cur_l: None,
        cur_r: R::No,
        stack: vec![Ent::Ch(first)],
        lig_present: false,
        lft_hit: false,
        rt_hit: false,
        ins_disc: false,
        bchar_on: true,
        hyphen,
        outer_h,
    };
    let mut pending: Option<Tok> = None;
    let mut lab = if cancel_boundary || !m.font.has_left_boundary() {
        m.cur_l = Some(first);
        L::Move2
    } else {
        m.cur_r = R::C(first);
        m.cur_l = None;
        L::LigLoop
    };
    let mut guard: u32 = 0;
    loop {
        guard += 1;
        if guard > 10_000_000 || (guard & 0xFFF == 0 && c.halted()) {
            break;
        }
        match lab {
            L::Wrapup => {
                let rt = m.rt_hit;
                m.wrapup(rt);
                lab = L::Move;
            }
            L::Move => {
                let Some(&top) = m.stack.last() else { break };
                m.q = m.out.len();
                m.cur_l = Some(top.ch());
                lab = L::Move1;
            }
            L::Move1 => match m.stack.last().copied() {
                Some(Ent::Lig { ptr, .. }) => {
                    // main_loop_move_lig
                    m.stack.pop();
                    if let Some(x) = ptr {
                        m.out.push(Item::Char { font: f, c: x });
                    }
                    m.lig_present = true;
                    match m.stack.last().copied() {
                        None => {
                            if ptr.is_some() {
                                lab = L::Lookahead;
                                continue;
                            }
                            m.cur_r = m.bchar();
                        }
                        Some(e) => m.cur_r = R::C(e.ch()),
                    }
                    lab = L::LigLoop;
                }
                _ => lab = L::Move2,
            },
            L::Move2 => {
                let Some(Ent::Ch(ch)) = m.stack.pop() else { break };
                let l = m.cur_l.unwrap_or(ch);
                if !m.font.has(ch) || !m.font.has(l) {
                    // char_warning; goto big_switch (no pending ligature is made)
                    let bad = if !m.font.has(ch) { ch } else { l };
                    for it in std::mem::take(&mut m.out) {
                        c.item(it);
                    }
                    char_warning(c, f, bad);
                    set_space_factor(c, sf);
                    return;
                }
                m.out.push(Item::Char { font: f, c: ch });
                lab = L::Lookahead;
            }
            L::Lookahead => {
                let t = c.next_x();
                let cls = match &t {
                    Some(t) => classify(c, t),
                    None => Cls::Other,
                };
                let ch = match cls {
                    Cls::Ch(ch) => Some(ch),
                    Cls::CharNum => Some(scan::scan_char_num(c)),
                    Cls::NoBoundary => {
                        m.bchar_on = false;
                        None
                    }
                    _ => None,
                };
                match ch {
                    Some(ch) => {
                        adjust_space_factor(c, &mut sf, ch);
                        m.stack.push(Ent::Ch(ch));
                        m.cur_r = if m.font.false_bchar() == Some(ch) { R::No } else { R::C(ch) };
                    }
                    None => {
                        pending = t;
                        m.cur_r = m.bchar();
                        m.stack.clear();
                    }
                }
                lab = L::LigLoop;
            }
            L::LigLoop => match m.lookup() {
                LigKern::None => lab = L::Wrapup,
                LigKern::Kern(w) => {
                    let rt = m.rt_hit;
                    m.wrapup(rt);
                    m.out.push(Item::Kern { w, sub: 0 });
                    lab = L::Move;
                }
                LigKern::Lig { op, c: lc } => {
                    if m.cur_l.is_none() {
                        m.lft_hit = true;
                    } else if m.stack.is_empty() {
                        m.rt_hit = true;
                    }
                    match op {
                        1 | 5 => {
                            m.cur_l = Some(lc);
                            m.lig_present = true;
                        }
                        2 | 6 => {
                            m.cur_r = R::C(lc);
                            match m.stack.last().copied() {
                                None => {
                                    // the right boundary character is consumed
                                    m.stack.push(Ent::Lig { c: lc, ptr: None });
                                    m.bchar_on = false;
                                }
                                Some(Ent::Ch(x)) => {
                                    *m.stack.last_mut().unwrap() = Ent::Lig { c: lc, ptr: Some(x) };
                                }
                                Some(Ent::Lig { ptr, .. }) => {
                                    *m.stack.last_mut().unwrap() = Ent::Lig { c: lc, ptr };
                                }
                            }
                        }
                        3 => {
                            m.cur_r = R::C(lc);
                            m.stack.push(Ent::Lig { c: lc, ptr: None });
                        }
                        7 | 11 => {
                            m.wrapup(false);
                            m.q = m.out.len();
                            m.cur_l = Some(lc);
                            m.lig_present = true;
                        }
                        _ => {
                            m.cur_l = Some(lc);
                            m.lig_present = true;
                            lab = if m.stack.is_empty() { L::Wrapup } else { L::Move1 };
                            continue;
                        }
                    }
                    lab = if op > 4 && op != 7 { L::Wrapup } else { L::LigLoop };
                }
            },
        }
    }
    for it in m.out {
        c.item(it);
    }
    set_space_factor(c, sf);
    if let Some(t) = pending {
        c.back(t);
    }
}

// ---------------------------------------------------------------- spaces

/// A space token in command position.
pub fn space_tok(c: &mut dyn Ctx) {
    if let Mode::Horizontal { .. } = c.mode() {
        if c.int(&Key::SpaceFactor) == 1000 {
            append_normal_space(c);
        } else {
            app_space(c);
        }
    }
}

fn font_glue(c: &mut dyn Ctx, f: FontId) -> Glue {
    Glue {
        width: fparam(c, f, 2),
        stretch: fparam(c, f, 3),
        shrink: fparam(c, f, 4),
        stretch_order: 0,
        shrink_order: 0,
    }
}

/// `append_normal_space` (tex.web 1041).
pub fn append_normal_space(c: &mut dyn Ctx) {
    let ss = gpar(c, "spaceskip");
    if ss == Glue::default() {
        let f = cur_font(c);
        let g = font_glue(c, f);
        c.item(glue_item(g, 0));
    } else {
        c.item(glue_item(ss, gp::of("spaceskip") as u8 + 1));
    }
}

/// `app_space` (tex.web 1043).
pub fn app_space(c: &mut dyn Ctx) {
    let sf = c.int(&Key::SpaceFactor);
    let xs = gpar(c, "xspaceskip");
    if sf >= 2000 && xs != Glue::default() {
        c.item(glue_item(xs, gp::of("xspaceskip") as u8 + 1));
        return;
    }
    let ss = gpar(c, "spaceskip");
    let f = cur_font(c);
    let mut g = if ss != Glue::default() { ss } else { font_glue(c, f) };
    if sf >= 2000 {
        g.width += fparam(c, f, 7);
    }
    g.stretch = xn_over_d(g.stretch, sf, 1000);
    g.shrink = xn_over_d(g.shrink, 1000, sf);
    c.item(glue_item(g, 0));
}

fn ex_space_cmd(c: &mut dyn Ctx, _code: u16) {
    if horizontal_here(c, " ") {
        append_normal_space(c);
    }
}

/// `\/` (tex.web 1113).
fn ital_corr_cmd(c: &mut dyn Ctx, _code: u16) {
    match c.mode() {
        Mode::Vertical { .. } => report_illegal_case(c, "/"),
        Mode::Horizontal { .. } => {
            let (f, ch) = match c.tail() {
                Some(Item::Char { font, c: ch }) => (font, ch),
                Some(Item::Lig { font, c: ch, .. }) => (font, ch),
                _ => return,
            };
            let w = fonts().font(f).italic(ch);
            c.item(Item::Kern { w, sub: 1 });
        }
        Mode::Math { .. } => c.item(Item::Kern { w: 0, sub: 1 }),
    }
}

// ---------------------------------------------------------------- accent

/// `\accent` (tex.web 1122–1125).
fn accent_cmd(c: &mut dyn Ctx, _code: u16) {
    match c.mode() {
        Mode::Vertical { .. } => {
            let t = self_tok(c, "accent");
            back_into_paragraph(c, t);
        }
        Mode::Math { .. } => crate::math::math_ac(c, true),
        Mode::Horizontal { .. } => make_accent(c),
    }
}

fn make_accent(c: &mut dyn Ctx) {
    let ach = scan::scan_char_num(c);
    let f = cur_font(c);
    let Some(mut p) = new_character(c, f, ach) else { return };
    let font = fonts().font(f);
    let x = fparam(c, f, 5);
    let s = fparam(c, f, 1) as f64 / 65536.0;
    let a = font.width(ach);
    do_assignments(c);
    let f2 = cur_font(c);
    let mut q = None;
    if let Some(t) = c.next_x() {
        match classify(c, &t) {
            Cls::Ch(ch) => q = new_character(c, f2, ch),
            Cls::CharNum => {
                let ch = scan::scan_char_num(c);
                q = new_character(c, f2, ch);
            }
            _ => c.back(t),
        }
    }
    if let Some(Item::Char { c: qc, .. }) = q {
        let font2 = fonts().font(f2);
        let t = fparam(c, f2, 1) as f64 / 65536.0;
        let w = font2.width(qc);
        let h = font2.height(qc);
        if h != x {
            p = Item::Box(Arc::new(BoxNode {
                width: a,
                height: font.height(ach),
                depth: font.depth(ach),
                shift: x - h,
                list: Arc::from(vec![p]),
                ..Default::default()
            }));
        }
        let delta = ((w - a) as f64 / 2.0 + h as f64 * t - x as f64 * s).round() as i32;
        c.item(Item::Kern { w: delta, sub: 2 });
        c.item(p);
        c.item(Item::Kern { w: -a - delta, sub: 2 });
        p = Item::Char { font: f2, c: qc };
    }
    c.item(p);
    set_space_factor(c, 1000);
}

/// Assignment commands that are `Kind::Command`s (`prefixed_command`'s range).
const ASSIGN_CMDS: &[&str] = &[
    "def", "edef", "gdef", "xdef", "let", "futurelet", "chardef", "mathchardef", "countdef",
    "dimendef", "skipdef", "muskipdef", "toksdef", "read", "readline", "catcode", "mathcode",
    "lccode", "uccode", "sfcode", "delcode", "textfont", "scriptfont", "scriptscriptfont",
    "advance", "multiply", "divide", "setbox", "font", "fontdimen", "hyphenchar", "skewchar",
    "hyphenation", "patterns", "parshape", "batchmode", "nonstopmode", "scrollmode",
    "errorstopmode", "interactionmode", "interlinepenalties", "clubpenalties", "widowpenalties",
    "displaywidowpenalties", "pdfcopyfont", "letterspacefont", "pdffontexpand", "lpcode",
    "rpcode", "efcode", "tagcode", "knbscode", "stbscode", "shbscode", "knbccode", "knaccode",
    "pdfnoligatures",
];

/// `do_assignments` (tex.web 1270): performs assignments up to the first
/// non-blank non-`\relax` token that is not one, which is put back.
pub fn do_assignments(c: &mut dyn Ctx) {
    loop {
        if c.halted() {
            return;
        }
        let Some(t) = c.next_x() else { return };
        let m = match &t {
            Tok::Char { cat: 10, .. } => continue,
            Tok::Char { .. } => {
                c.back(t);
                return;
            }
            _ => c.meaning(&t),
        };
        match m {
            Meaning::Char { cat: 10, .. } => continue,
            Meaning::Font(_) | Meaning::Reg(..) => crate::assign::assign_meaning(c, t, 0),
            Meaning::Prim(id) => {
                let d = *registry().get(id);
                if d.name == "relax" {
                    continue;
                }
                match d.kind {
                    Kind::Prefix => crate::assign::prefix(c, d.code),
                    Kind::Quantity { .. } | Kind::Computed { set: Some(_), .. } => {
                        crate::assign::prefixed_command(c, id, 0)
                    }
                    Kind::Command(_) if ASSIGN_CMDS.contains(&d.name) => crate::assign::prefixed_command(c, id, 0),
                    _ => {
                        c.back(t);
                        return;
                    }
                }
            }
            _ => {
                c.back(t);
                return;
            }
        }
    }
}

// ---------------------------------------------------------------- glue, kern, penalty, rules

fn skip_glue(c: &mut dyn Ctx, code: u16) -> Glue {
    let u = 65536;
    match code {
        0 => Glue { stretch: u, stretch_order: 1, ..Default::default() },
        1 => Glue { stretch: u, stretch_order: 2, ..Default::default() },
        2 => Glue { stretch: u, stretch_order: 1, shrink: u, shrink_order: 1, ..Default::default() },
        3 => Glue { stretch: -u, stretch_order: 1, ..Default::default() },
        _ => scan::scan_glue(c, Level::Glue),
    }
}

const HSKIP_NAMES: [&str; 5] = ["hfil", "hfill", "hss", "hfilneg", "hskip"];
const VSKIP_NAMES: [&str; 5] = ["vfil", "vfill", "vss", "vfilneg", "vskip"];

fn hskip_cmd(c: &mut dyn Ctx, code: u16) {
    if horizontal_here(c, HSKIP_NAMES[code.min(4) as usize]) {
        let g = skip_glue(c, code);
        c.item(glue_item(g, 0));
    }
}

fn vskip_cmd(c: &mut dyn Ctx, code: u16) {
    if vertical_here(c, VSKIP_NAMES[code.min(4) as usize], false) {
        let g = skip_glue(c, code);
        c.item(glue_item(g, 0));
    }
}

fn kern_cmd(c: &mut dyn Ctx, _code: u16) {
    let w = scan::scan_dimen(c, false, false).0;
    c.item(Item::Kern { w, sub: 1 });
}

fn penalty_cmd(c: &mut dyn Ctx, _code: u16) {
    let p = scan::scan_int(c);
    c.item(Item::Penalty(p));
}

/// `scan_rule_spec` (tex.web 463).
pub fn scan_rule_spec(c: &mut dyn Ctx, vrule: bool) -> Item {
    let (mut w, mut h, mut d) = (RUNNING, RUNNING, RUNNING);
    if vrule {
        w = DEFAULT_RULE;
    } else {
        h = DEFAULT_RULE;
        d = 0;
    }
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

fn hrule_cmd(c: &mut dyn Ctx, _code: u16) {
    if vertical_here(c, "hrule", true) {
        let r = scan_rule_spec(c, false);
        c.item(r);
        c.define(Key::PrevDepth, Val::Int(IGNORE_DEPTH), false);
    }
}

fn vrule_cmd(c: &mut dyn Ctx, _code: u16) {
    if horizontal_here(c, "vrule") {
        let r = scan_rule_spec(c, true);
        c.item(r);
        if is_h(c.mode()) {
            set_space_factor(c, 1000);
        }
    }
}

// ---------------------------------------------------------------- \unskip & co, \lastskip & co

const REMOVE_NAMES: [&str; 3] = ["unpenalty", "unkern", "unskip"];

/// `delete_last` (tex.web 1105, e-TeX's apology).
fn remove_item_cmd(c: &mut dyn Ctx, code: u16) {
    let m = c.mode();
    let tail = c.tail();
    if m == (Mode::Vertical { inner: false }) && tail.is_none() {
        let last_is_glue = crate::page::last_glue(c).is_some();
        if code != 2 || last_is_glue {
            let help: [&str; 2] = [
                "Sorry...I usually can't take things from the current page.",
                match code {
                    1 => "Try `I\\kern-\\lastkern' instead.",
                    2 => "Try `I\\vskip-\\lastskip' instead.",
                    _ => "Perhaps you can make the output routine do it.",
                },
            ];
            you_cant(c, REMOVE_NAMES[code.min(2) as usize], &help);
        }
        return;
    }
    let hit = matches!(
        (&tail, code),
        (Some(Item::Penalty(_)), 0) | (Some(Item::Kern { .. }), 1) | (Some(Item::Glue { .. }), 2)
    );
    if hit {
        c.emit(Emit::RemoveTail(code as u8));
    }
}

/// tex.web node types (char nodes: -1).
fn node_type(it: &Item) -> i32 {
    match it {
        Item::Char { .. } => -1,
        Item::Box(b) => i32::from(b.vertical),
        Item::Rule { .. } => 2,
        Item::Ins(_) => 3,
        Item::Mark { .. } => 4,
        Item::Adjust(_) => 5,
        Item::Lig { .. } => 6,
        Item::Disc(_) => 7,
        Item::Whatsit(_) => 8,
        Item::Math { .. } => 9,
        Item::Glue { .. } => 10,
        Item::Kern { .. } | Item::MarginKern { .. } => 11,
        Item::Penalty(_) => 12,
        Item::Unset(_) => 13,
        Item::Noad(_) => 14,
    }
}

/// `\lastpenalty` `\lastkern` `\lastskip` `\lastnodetype` (tex.web 424, e-TeX).
fn last_item_get(c: &mut dyn Ctx, code: u16) -> Val {
    let m = c.mode();
    let tail = c.tail();
    if m == (Mode::Vertical { inner: false }) && tail.is_none() {
        return match code {
            0 => Val::Int(crate::page::last_penalty(c)),
            1 => Val::Int(crate::page::last_kern(c)),
            2 => Val::Glue(crate::page::last_glue(c).unwrap_or_default()),
            _ => Val::Int(crate::page::last_node_type(c)),
        };
    }
    match (code, &tail) {
        (0, Some(Item::Penalty(p))) => Val::Int(*p),
        (0, _) => Val::Int(0),
        (1, Some(Item::Kern { w, .. })) => Val::Int(*w),
        (1, _) => Val::Int(0),
        (2, Some(Item::Glue { g, .. })) => Val::Glue(*g),
        (2, _) => Val::Glue(Glue::default()),
        (_, None) => Val::Int(-1),
        (_, Some(it)) => Val::Int(node_type(it) + 1),
    }
}

/// `\spacefactor` (0) / `\prevdepth` (1).
fn aux_get(c: &mut dyn Ctx, code: u16) -> Val {
    let m = c.mode();
    let ok = if code == 0 { is_h(m) } else { is_v(m) };
    if !ok {
        let n = esc(c, if code == 0 { "spacefactor" } else { "prevdepth" });
        error(
            c,
            &format!("Improper {n}"),
            &[
                "You can refer to \\spacefactor only in horizontal mode;",
                "you can refer to \\prevdepth only in vertical mode; and",
                "neither of these is meaningful inside \\write. So",
                "I'm forgetting what you said and using zero instead.",
            ],
        );
        return Val::Int(0);
    }
    Val::Int(c.int(if code == 0 { &Key::SpaceFactor } else { &Key::PrevDepth }))
}

/// `alter_aux` (tex.web 1243).
fn aux_set(c: &mut dyn Ctx, code: u16, v: Val, _global: bool) {
    let m = c.mode();
    let ok = if code == 0 { is_h(m) } else { is_v(m) };
    if !ok {
        report_illegal_case(c, if code == 0 { "spacefactor" } else { "prevdepth" });
        return;
    }
    let n = match v {
        Val::Glue(g) => g.width,
        v => v.int(),
    };
    if code == 1 {
        c.define(Key::PrevDepth, Val::Int(n), false);
    } else if n <= 0 || n > 32767 {
        error(c, &format!("Bad space factor ({n})"), &["I allow only values in the range 1..32767 here."]);
    } else {
        set_space_factor(c, n);
    }
}

fn prev_graf_get(c: &mut dyn Ctx, _code: u16) -> Val {
    Val::Int(c.int(&Key::PrevGraf))
}

/// `alter_prev_graf` (tex.web 1244).
fn prev_graf_set(c: &mut dyn Ctx, _code: u16, v: Val, _global: bool) {
    let n = v.int();
    if n < 0 {
        let e = esc(c, "prevgraf");
        error(c, &format!("Bad {e} ({n})"), &["I allow only nonnegative values here."]);
    } else {
        c.define(Key::PrevGraf, Val::Int(n), false);
    }
}

// ---------------------------------------------------------------- discretionaries

#[derive(Debug, PartialEq)]
pub struct DiscState {
    pub part: u8,
    pub pre: Arc<[Item]>,
    pub post: Arc<[Item]>,
}

impl Datum for DiscState {
    fn same(&self, o: &dyn Datum) -> bool {
        o.as_any().downcast_ref::<DiscState>().is_some_and(|x| x == self)
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn empty_items() -> Arc<[Item]> {
    Arc::from(Vec::new())
}

fn open_disc_part(c: &mut dyn Ctx, st: DiscState) {
    c.open_group(GroupKind::Disc);
    c.define(Key::Other(tag::LIST, other::DISC), Val::Data(Arc::new(st)), false);
    scan::scan_left_brace(c);
    c.open_list(Mode::Horizontal { inner: true });
    set_space_factor(c, 1000);
}

/// `\-` (1) and `\discretionary` (0) (tex.web 1117).
fn disc_cmd(c: &mut dyn Ctx, code: u16) {
    if !horizontal_here(c, if code == 1 { "-" } else { "discretionary" }) {
        return;
    }
    if code == 1 {
        let f = cur_font(c);
        let h = crate::font::hyphen_char(c, f);
        let mut pre = Vec::new();
        if (0..256).contains(&h) {
            if let Some(it) = new_character(c, f, h as u16) {
                pre.push(it);
            }
        }
        c.item(Item::Disc(Arc::new(Disc { pre: Arc::from(pre), post: empty_items(), replace: 0, explicit: true })));
    } else {
        open_disc_part(c, DiscState { part: 0, pre: empty_items(), post: empty_items() });
    }
}

/// `build_discretionary` (tex.web 1119).
fn build_discretionary(c: &mut dyn Ctx) {
    let st = match c.get(&Key::Other(tag::LIST, other::DISC)) {
        Val::Data(d) => d
            .as_any()
            .downcast_ref::<DiscState>()
            .map(|s| DiscState { part: s.part, pre: s.pre.clone(), post: s.post.clone() }),
        _ => None,
    }
    .unwrap_or(DiscState { part: 0, pre: empty_items(), post: empty_items() });
    c.close_group();
    let mut items = c.close_list();
    if let Some(i) = items.iter().position(|it| {
        !matches!(it, Item::Char { .. } | Item::Lig { .. } | Item::Kern { .. } | Item::Box(_) | Item::Rule { .. })
    }) {
        error(c, "Improper discretionary list", &["Discretionary lists must contain only boxes and kerns."]);
        let line = c.line_no();
        c.emit(Emit::Diag(Arc::new(Diag {
            msg: "The following discretionary sublist has been deleted:".into(),
            help: Vec::new(),
            line,
        })));
        items.truncate(i);
    }
    match st.part {
        0 => open_disc_part(c, DiscState { part: 1, pre: Arc::from(items), post: st.post }),
        1 => open_disc_part(c, DiscState { part: 2, pre: st.pre, post: Arc::from(items) }),
        _ => {
            let mut n = items.len();
            if n > 0 && is_m(c.mode()) {
                let e = esc(c, "discretionary");
                error(
                    c,
                    &format!("Illegal math {e}"),
                    &["Sorry: The third part of a discretionary break must be", "empty, in math formulas. I had to delete your third part."],
                );
                items.clear();
                n = 0;
            }
            let replace = if n <= 255 {
                n as u16
            } else {
                error(
                    c,
                    "Discretionary list is too long",
                    &["Wow---I never thought anybody would tweak me here.", "You can't seriously need such a huge discretionary list?"],
                );
                0
            };
            c.item(Item::Disc(Arc::new(Disc { pre: st.pre, post: st.post, replace, explicit: true })));
            for it in items {
                c.item(it);
            }
        }
    }
}

// ---------------------------------------------------------------- \insert \vadjust \mark

/// `normal_paragraph` (tex.web 1070, e-TeX).
pub fn normal_paragraph(c: &mut dyn Ctx) {
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

/// `begin_insert_or_adjust` (tex.web 1099): `\insert` (0), `\vadjust` (1).
fn insert_cmd(c: &mut dyn Ctx, code: u16) {
    if code == 1 && is_v(c.mode()) {
        report_illegal_case(c, "vadjust");
        return;
    }
    let class: i32 = if code == 1 {
        255
    } else {
        let n = scan::scan_eight_bit_int(c) as i32;
        if n == 255 {
            let e = esc(c, "insert");
            error(c, &format!("You can't {e}255"), &["I'm changing to \\insert0; box 255 is special."]);
            0
        } else {
            n
        }
    };
    c.open_group(GroupKind::Insert);
    c.define(Key::Other(tag::LIST, other::INSERT), Val::Int(class), false);
    scan::scan_left_brace(c);
    normal_paragraph(c);
    c.open_list(Mode::Vertical { inner: true });
    c.define(Key::PrevDepth, Val::Int(IGNORE_DEPTH), false);
}

/// An insert group ends (tex.web 1100).
fn insert_end(c: &mut dyn Ctx) {
    end_graf(c);
    let q = gpar(c, "splittopskip");
    let d = dpar(c, "splitmaxdepth");
    let f = ipar(c, "floatingpenalty");
    let class = c.int(&Key::Other(tag::LIST, other::INSERT));
    c.close_group();
    let items = c.close_list();
    let p = crate::pack::vpack(items, crate::pack::Spec::Additional(0), MAX_DIMEN, &crate::pack::Params::default());
    if class < 255 {
        c.item(Item::Ins(Arc::new(Ins {
            class: class.clamp(0, 254) as u8,
            height: p.b.height + p.b.depth,
            depth: d,
            split_top: q,
            split_max_depth: d,
            float_cost: f,
            list: p.b.list.clone(),
        })));
    } else {
        c.item(Item::Adjust(p.b.list.clone()));
    }
}

/// `\mark` (0), e-TeX `\marks` (1) (tex.web 1101).
fn mark_cmd(c: &mut dyn Ctx, code: u16) {
    let class = if code == 0 { 0 } else { scan::scan_register_num(c) };
    let (_, toks) = scan::scan_toks(c, false, true);
    c.item(Item::Mark { class, toks });
}

/// A group owned here ends (`Disc`, `Insert`); called by `groups::right_brace`.
/// Closes the group itself.
pub fn group_end(c: &mut dyn Ctx, kind: GroupKind) {
    match kind {
        GroupKind::Disc => build_discretionary(c),
        GroupKind::Insert => insert_end(c),
        _ => c.close_group(),
    }
}

// ---------------------------------------------------------------- paragraphs

fn start_par_cmd(c: &mut dyn Ctx, code: u16) {
    match c.mode() {
        Mode::Vertical { .. } => new_graf(c, code > 0),
        m => {
            // indent_in_hmode (tex.web 1093)
            if code > 0 {
                let w = dpar(c, "parindent");
                let b = Arc::new(BoxNode { width: w, ..Default::default() });
                if is_h(m) {
                    c.item(Item::Box(b));
                    set_space_factor(c, 1000);
                } else {
                    crate::math::box_noad(c, b);
                }
            }
        }
    }
}

/// A paragraph begins (letter or `\indent` in vertical mode) (tex.web 1091).
pub fn new_graf(c: &mut dyn Ctx, indent: bool) {
    let m = c.mode();
    c.define(Key::PrevGraf, Val::Int(0), false);
    if m == (Mode::Vertical { inner: false }) || c.tail().is_some() {
        let g = param_glue(c, "parskip");
        c.item(g);
    }
    let lang = cur_lang(c);
    let lhm = norm_min(ipar(c, "lefthyphenmin"));
    let rhm = norm_min(ipar(c, "righthyphenmin"));
    let parindent = if indent { dpar(c, "parindent") } else { 0 };
    let every = c.get(&Key::TokPar(crate::prim::tp::of("everypar"))).toks();
    c.open_list(Mode::Horizontal { inner: false });
    set_space_factor(c, 1000);
    c.define(Key::Clang, Val::Int(lang), false);
    c.define(Key::PrevDepth, Val::Int((lhm * 64 + rhm) * 65536 + lang), false);
    c.define(Key::PrevGraf, Val::Int(0), false);
    if indent {
        c.item(null_box(parindent));
    }
    if !every.is_empty() {
        c.push(every, TokKind::EveryPar);
    }
}

fn par_cmd(c: &mut dyn Ctx, _code: u16) {
    match c.mode() {
        Mode::Vertical { .. } => normal_paragraph(c),
        Mode::Horizontal { .. } => end_graf(c),
        Mode::Math { .. } => {
            let t = self_tok(c, "par");
            insert_dollar_sign(c, t);
        }
    }
}

/// The paragraph ends (`\par` in horizontal mode, or a vertical command)
/// (tex.web 1096). Acts only in (unrestricted) horizontal mode.
pub fn end_graf(c: &mut dyn Ctx) {
    if c.mode() == (Mode::Horizontal { inner: false }) {
        finish_paragraph(c, false);
        normal_paragraph(c);
    }
}

/// How a paragraph ended.
pub enum Para {
    /// It was empty (`head=tail`): nothing was emitted.
    Empty,
    /// Broken into lines, emitted to the enclosing vertical list; its last line.
    Broken(Option<Arc<BoxNode>>),
}

/// `line_break(display_widow_penalty)` before a display: the paragraph's
/// last line (`None`: the paragraph was empty).
pub fn display_break(c: &mut dyn Ctx) -> Option<Arc<BoxNode>> {
    match finish_paragraph(c, true) {
        Para::Broken(l) => l,
        Para::Empty => None,
    }
}

/// Closes the current paragraph list and breaks it into lines appended to the
/// enclosing vertical list (`line_break(widow_penalty)`, or with `display`
/// `line_break(display_widow_penalty)` before a display). Sets `\prevgraf`
/// and `\prevdepth` of the enclosing list.
pub fn finish_paragraph(c: &mut dyn Ctx, display: bool) -> Para {
    let packed = c.int(&Key::PrevDepth);
    let pg = c.int(&Key::PrevGraf);
    let items = c.close_list();
    if items.is_empty() {
        c.define(Key::PrevGraf, Val::Int(pg), false);
        return Para::Empty;
    }
    let prev_depth = c.int(&Key::PrevDepth);
    let p = line_params(c, &items, display, packed, pg, prev_depth);
    let lines = crate::linebreak::line_break(items, &p, fonts());
    for it in lines.items {
        c.item(it);
    }
    c.define(Key::PrevGraf, Val::Int(lines.prev_graf), false);
    if let Some(b) = &lines.last {
        c.define(Key::PrevDepth, Val::Int(b.depth), false);
    }
    Para::Broken(lines.last)
}

fn collect(items: &[Item], fs: &mut Vec<FontId>, chars: &mut Vec<u16>, langs: &mut Vec<u8>) {
    fn add<T: PartialEq + Copy>(v: &mut Vec<T>, x: T) {
        if !v.contains(&x) {
            v.push(x);
        }
    }
    for it in items {
        match it {
            Item::Char { font, c } => {
                add(fs, *font);
                add(chars, *c);
            }
            Item::Lig { font, c, orig, .. } => {
                add(fs, *font);
                add(chars, *c);
                for x in orig.iter() {
                    add(chars, *x);
                }
            }
            Item::Disc(d) => {
                collect(&d.pre, fs, chars, langs);
                collect(&d.post, fs, chars, langs);
            }
            Item::Whatsit(w) => {
                if let Whatsit::Language { lang, .. } = **w {
                    add(langs, lang);
                }
            }
            _ => {}
        }
    }
}

fn ints(c: &mut dyn Ctx, k: Key) -> Arc<[i32]> {
    match c.get(&k) {
        Val::Ints(v) => v,
        _ => Arc::from(Vec::new()),
    }
}

/// Everything line breaking reads, gathered here (C3's `LineParams`).
pub fn line_params(
    c: &mut dyn Ctx,
    items: &[Item],
    display: bool,
    packed_lang: i32,
    prev_graf: i32,
    prev_depth: i32,
) -> crate::linebreak::LineParams {
    let init_lang = (packed_lang % 65536).clamp(0, 255) as u8;
    let init_lhm = ((packed_lang / 65536) / 64).clamp(0, 63) as u8;
    let init_rhm = ((packed_lang / 65536) % 64).clamp(0, 63) as u8;
    let mut fs = Vec::new();
    let mut chars = Vec::new();
    let mut lang_ids = vec![init_lang];
    collect(items, &mut fs, &mut chars, &mut lang_ids);
    let mut langs = Vec::new();
    for l in lang_ids {
        let p = match c.get(&Key::Patterns(l)) {
            Val::Patterns(p) => Some(p),
            _ => None,
        };
        let e = match c.get(&Key::Exceptions(l)) {
            Val::Exceptions(e) => Some(e),
            _ => None,
        };
        langs.push((l, p, e));
    }
    let hyphen_chars: Vec<(FontId, i32)> = fs.iter().map(|&f| (f, crate::font::hyphen_char(c, f))).collect();
    let lc_codes: Vec<(u16, u16)> = chars.iter().map(|&ch| (ch, c.int(&Key::Lc(key_char(ch))) as u16)).collect();
    let adjust_spacing = if c.pdf() { ipar(c, "pdfadjustspacing") } else { 0 };
    let protrude_chars = if c.pdf() { ipar(c, "pdfprotrudechars") } else { 0 };
    let mut font_codes = Vec::new();
    if adjust_spacing > 0 || protrude_chars > 0 {
        for &f in &fs {
            for &ch in &chars {
                for which in [crate::font::code::LP, crate::font::code::RP, crate::font::code::EF] {
                    let v = crate::font::font_code(c, which, f, ch);
                    font_codes.push((which, f, ch, v));
                }
            }
        }
    }
    let par_shape = match c.get(&Key::Parshape) {
        Val::Parshape(p) => p,
        _ => Arc::from(Vec::new()),
    };
    let etex = c.etex();
    let empty = || -> Arc<[i32]> { Arc::from(Vec::new()) };
    let mut lp_codes = std::collections::BTreeMap::new();
    let mut rp_codes = std::collections::BTreeMap::new();
    for (which, f, ch, v) in font_codes {
        if which == crate::font::code::LP {
            lp_codes.insert((f, ch), v);
        } else if which == crate::font::code::RP {
            rp_codes.insert((f, ch), v);
        }
    }
    crate::linebreak::LineParams {
        pretolerance: ipar(c, "pretolerance"),
        tolerance: ipar(c, "tolerance"),
        emergency_stretch: dpar(c, "emergencystretch"),
        looseness: ipar(c, "looseness"),
        line_penalty: ipar(c, "linepenalty"),
        hyphen_penalty: ipar(c, "hyphenpenalty"),
        ex_hyphen_penalty: ipar(c, "exhyphenpenalty"),
        adj_demerits: ipar(c, "adjdemerits"),
        double_hyphen_demerits: ipar(c, "doublehyphendemerits"),
        final_hyphen_demerits: ipar(c, "finalhyphendemerits"),
        inter_line_penalty: ipar(c, "interlinepenalty"),
        club_penalty: ipar(c, "clubpenalty"),
        widow_penalty: ipar(c, "widowpenalty"),
        display_widow_penalty: ipar(c, "displaywidowpenalty"),
        broken_penalty: ipar(c, "brokenpenalty"),
        inter_line_penalties: if etex { ints(c, Key::PenShape(0)) } else { empty() },
        club_penalties: if etex { ints(c, Key::PenShape(1)) } else { empty() },
        widow_penalties: if etex { ints(c, Key::PenShape(2)) } else { empty() },
        display_widow_penalties: if etex { ints(c, Key::PenShape(3)) } else { empty() },
        display,
        hsize: dpar(c, "hsize"),
        hang_indent: dpar(c, "hangindent"),
        hang_after: ipar(c, "hangafter"),
        parshape: par_shape,
        left_skip: gpar(c, "leftskip"),
        right_skip: gpar(c, "rightskip"),
        par_fill_skip: gpar(c, "parfillskip"),
        baseline_skip: gpar(c, "baselineskip"),
        line_skip: gpar(c, "lineskip"),
        line_skip_limit: dpar(c, "lineskiplimit"),
        prev_depth,
        prev_graf,
        last_line_fit: if etex { ipar(c, "lastlinefit") } else { 0 },
        uc_hyph: ipar(c, "uchyph"),
        lang: i32::from(init_lang),
        lhm: i32::from(init_lhm),
        rhm: i32::from(init_rhm),
        langs: langs.into_iter().map(|(l, p, e)| (l, (p, e))).collect(),
        hyphen_chars: hyphen_chars.into_iter().collect(),
        lc: lc_codes.into_iter().collect(),
        adjust_spacing,
        protrude_chars,
        lp_codes,
        rp_codes,
        pack: crate::boxes::read_params(c),
    }
}

/// Appends a box to a vertical list with interline glue (`append_to_vlist`, tex.web 679).
pub fn append_to_vlist(c: &mut dyn Ctx, b: Arc<BoxNode>) {
    let pd = c.int(&Key::PrevDepth);
    if pd > IGNORE_DEPTH {
        let bs = gpar(c, "baselineskip");
        let d = bs.width - pd - b.height;
        if d < dpar(c, "lineskiplimit") {
            let g = param_glue(c, "lineskip");
            c.item(g);
        } else {
            let mut g = bs;
            g.width = d;
            c.item(glue_item(g, gp::of("baselineskip") as u8 + 1));
        }
    }
    let depth = b.depth;
    c.item(Item::Box(b));
    c.define(Key::PrevDepth, Val::Int(depth), false);
}

// ---------------------------------------------------------------- \end \dump

/// `\end` (0) / `\dump` (1): `its_all_over` (tex.web 1054).
fn stop_cmd(c: &mut dyn Ctx, code: u16) {
    let name = if code == 1 { "dump" } else { "end" };
    let m = c.mode();
    if is_h(m) {
        let t = self_tok(c, name);
        head_for_vmode(c, t, false);
        return;
    }
    if is_m(m) {
        let t = self_tok(c, name);
        insert_dollar_sign(c, t);
        return;
    }
    if m != (Mode::Vertical { inner: false }) {
        report_illegal_case(c, name);
        return;
    }
    let dead = c.int(&Key::Page(8));
    if crate::page::page_empty(c) && c.tail().is_none() && dead == 0 {
        c.define(Key::Ended, Val::Int(1), true);
        return;
    }
    let t = self_tok(c, name);
    c.back(t);
    let hsize = dpar(c, "hsize");
    c.item(null_box(hsize));
    c.item(glue_item(Glue { stretch: 65536, stretch_order: 2, ..Default::default() }, 0));
    c.item(Item::Penalty(-0o10000000000));
}
