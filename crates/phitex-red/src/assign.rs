//! B5: assignments as deltas: prefixes, registers, parameters, `\let`
//! `\futurelet` `\chardef`… `\advance` `\multiply` `\divide`, code tables,
//! `\afterassignment`, `\globaldefs`, `\setbox` (box from `boxes.rs`), `\parshape`.
//!
//! tex.web 1208–1280 (`prefixed_command`). Every change is a `ctx.define`;
//! no state lives here.
//!
//! Entry points for the reducer:
//! * a `Kind::Prefix` token → [`prefix`] (gathers more prefixes, then assigns);
//! * a `Kind::Quantity` token, or a `Kind::Computed` with a setter, in command
//!   position → [`prefixed_command`] with prefixes 0;
//! * a token meaning `Meaning::Font` / `Meaning::Reg` in command position →
//!   [`assign_meaning`] with prefixes 0;
//! * this module's own commands are `Kind::Command` and come here directly.
//!
//! Setter convention for `Kind::Computed { set: Some(f) }` owned by other
//! modules (`\wd`, `\spacefactor`, `\interactionmode`, …): `f(ctx, code,
//! Val::Void, global)` is called right after the command token; the setter
//! scans its own operands, the optional `=` and the value (only it knows
//! its operand syntax).
use std::sync::{Arc, OnceLock};

use crate::arith;
use crate::ctx::Ctx;
use crate::diag::error;
use crate::prim::{dp, gp, ip, mp, registry, tp, Def, Kind, Level, PrimId, Registry};
use crate::scan;
use crate::term::TokKind;
use crate::tok::{Tok, Toks};
use crate::val::{FontId, Glue, Key, Macro, Meaning, Val};

// ---- registration

// Command codes (this module's own `Kind::Command`s).
const LET: u16 = 0;
const FUTURELET: u16 = 1;
const CHARDEF: u16 = 10;
const MATHCHARDEF: u16 = 11;
const COUNTDEF: u16 = 12;
const DIMENDEF: u16 = 13;
const SKIPDEF: u16 = 14;
const MUSKIPDEF: u16 = 15;
const TOKSDEF: u16 = 16;
const ADVANCE: u16 = 20;
const MULTIPLY: u16 = 21;
const DIVIDE: u16 = 22;
const SETBOX: u16 = 30;
const AFTERASSIGNMENT: u16 = 40;

/// This module's commands that are assignments (prefixable).
const MINE: &[&str] = &[
    "let", "futurelet", "chardef", "mathchardef", "countdef", "dimendef", "skipdef", "muskipdef",
    "toksdef", "advance", "multiply", "divide", "setbox",
];
/// This module's computed quantities assigned with their own syntax.
const SHAPES: &[&str] = &["parshape", "interlinepenalties", "clubpenalties", "widowpenalties", "displaywidowpenalties"];
/// `\def` & co (B2's): prefixes go to `macros::define_macro`.
const DEFS: &[&str] = &["def", "gdef", "edef", "xdef"];
/// Other modules' commands that are prefixable assignments in tex.web
/// (the prefix is accepted; they assign as they always do).
const OTHERS: &[&str] = &[
    "read", "readline", "font", "hyphenation", "patterns", "batchmode", "nonstopmode", "scrollmode",
    "errorstopmode", "letcharcode", "pdfglyphtounicode",
];

pub fn register(r: &mut Registry) {
    r.add("global", 1, Kind::Prefix, 0);
    r.add("long", 2, Kind::Prefix, 0);
    r.add("outer", 4, Kind::Prefix, 0);
    r.add("protected", 8, Kind::Prefix, 1);

    r.add("let", LET, Kind::Command(cmd), 0);
    r.add("futurelet", FUTURELET, Kind::Command(cmd), 0);
    r.add("chardef", CHARDEF, Kind::Command(cmd), 0);
    r.add("mathchardef", MATHCHARDEF, Kind::Command(cmd), 0);
    r.add("countdef", COUNTDEF, Kind::Command(cmd), 0);
    r.add("dimendef", DIMENDEF, Kind::Command(cmd), 0);
    r.add("skipdef", SKIPDEF, Kind::Command(cmd), 0);
    r.add("muskipdef", MUSKIPDEF, Kind::Command(cmd), 0);
    r.add("toksdef", TOKSDEF, Kind::Command(cmd), 0);
    r.add("advance", ADVANCE, Kind::Command(cmd), 0);
    r.add("multiply", MULTIPLY, Kind::Command(cmd), 0);
    r.add("divide", DIVIDE, Kind::Command(cmd), 0);
    r.add("setbox", SETBOX, Kind::Command(cmd), 0);
    r.add("afterassignment", AFTERASSIGNMENT, Kind::Command(after_assignment_cmd), 0);

    // registers
    r.add("count", 0, Kind::Quantity { level: Level::Int, key: reg_key }, 0);
    r.add("dimen", 1, Kind::Quantity { level: Level::Dimen, key: reg_key }, 0);
    r.add("skip", 2, Kind::Quantity { level: Level::Glue, key: reg_key }, 0);
    r.add("muskip", 3, Kind::Quantity { level: Level::Mu, key: reg_key }, 0);
    r.add("toks", 4, Kind::Quantity { level: Level::Tok, key: reg_key }, 0);

    // parameters
    let first_etex_int = ip::of("tracingassigns") as usize;
    let first_pdf_int = ip::of("pdfoutput") as usize;
    for (i, n) in ip::NAMES.iter().enumerate() {
        let e = if i >= first_pdf_int { 2 } else if i >= first_etex_int { 1 } else { 0 };
        r.add(n, i as u16, Kind::Quantity { level: Level::Int, key: int_par }, e);
    }
    let first_pdf_dimen = dp::of("pdfhorigin") as usize;
    for (i, n) in dp::NAMES.iter().enumerate() {
        let e = if i >= first_pdf_dimen { 2 } else { 0 };
        r.add(n, i as u16, Kind::Quantity { level: Level::Dimen, key: dimen_par }, e);
    }
    for (i, n) in gp::NAMES.iter().enumerate() {
        r.add(n, i as u16, Kind::Quantity { level: Level::Glue, key: glue_par }, 0);
    }
    for (i, n) in mp::NAMES.iter().enumerate() {
        r.add(n, i as u16, Kind::Quantity { level: Level::Mu, key: mu_par }, 0);
    }
    let first_etex_tok = tp::of("everyeof") as usize;
    let first_pdf_tok = tp::of("pdfpagesattr") as usize;
    for (i, n) in tp::NAMES.iter().enumerate() {
        let e = if i >= first_pdf_tok { 2 } else if i >= first_etex_tok { 1 } else { 0 };
        r.add(n, i as u16, Kind::Quantity { level: Level::Tok, key: tok_par }, e);
    }

    // code tables
    for (i, n) in ["catcode", "lccode", "uccode", "sfcode", "mathcode", "delcode"].iter().enumerate() {
        r.add(n, i as u16, Kind::Quantity { level: Level::Int, key: code_key }, 0);
    }
    // math font families
    for (i, n) in ["textfont", "scriptfont", "scriptscriptfont"].iter().enumerate() {
        r.add(n, i as u16, Kind::Quantity { level: Level::Ident, key: fam_key }, 0);
    }

    // shapes
    r.add("parshape", 0, Kind::Computed { level: Level::Int, get: shape_get, set: None }, 0);
    r.add("interlinepenalties", 1, Kind::Computed { level: Level::Int, get: shape_get, set: None }, 1);
    r.add("clubpenalties", 2, Kind::Computed { level: Level::Int, get: shape_get, set: None }, 1);
    r.add("widowpenalties", 3, Kind::Computed { level: Level::Int, get: shape_get, set: None }, 1);
    r.add("displaywidowpenalties", 4, Kind::Computed { level: Level::Int, get: shape_get, set: None }, 1);
    r.add("parshapelength", 0, Kind::Computed { level: Level::Dimen, get: parshape_dimen, set: None }, 1);
    r.add("parshapeindent", 1, Kind::Computed { level: Level::Dimen, get: parshape_dimen, set: None }, 1);
    r.add("parshapedimen", 2, Kind::Computed { level: Level::Dimen, get: parshape_dimen, set: None }, 1);
}

fn reg_key(c: &mut dyn Ctx, code: u16) -> Key {
    let n = scan::scan_register_num(c);
    bank_key(code as u8, n)
}

fn bank_key(bank: u8, n: u16) -> Key {
    match bank {
        0 => Key::Count(n),
        1 => Key::Dimen(n),
        2 => Key::Skip(n),
        3 => Key::Muskip(n),
        _ => Key::Toks(n),
    }
}

fn bank_level(bank: u8) -> Level {
    match bank {
        0 => Level::Int,
        1 => Level::Dimen,
        2 => Level::Glue,
        3 => Level::Mu,
        _ => Level::Tok,
    }
}

fn int_par(_c: &mut dyn Ctx, code: u16) -> Key {
    Key::IntPar(code)
}
fn dimen_par(_c: &mut dyn Ctx, code: u16) -> Key {
    Key::DimenPar(code)
}
fn glue_par(_c: &mut dyn Ctx, code: u16) -> Key {
    Key::GluePar(code)
}
fn mu_par(_c: &mut dyn Ctx, code: u16) -> Key {
    Key::MuGluePar(code)
}
fn tok_par(_c: &mut dyn Ctx, code: u16) -> Key {
    Key::TokPar(code)
}

fn chr(n: u16) -> char {
    char::from_u32(n as u32).unwrap_or('\0')
}

fn code_key(c: &mut dyn Ctx, code: u16) -> Key {
    let ch = chr(scan::scan_char_num(c));
    match code {
        0 => Key::Cat(ch),
        1 => Key::Lc(ch),
        2 => Key::Uc(ch),
        3 => Key::Sf(ch),
        4 => Key::MathCode(ch),
        _ => Key::DelCode(ch),
    }
}

fn fam_key(c: &mut dyn Ctx, code: u16) -> Key {
    let f = scan::scan_four_bit_int(c);
    Key::FamFont(code as u8, f)
}

fn shape_key(code: u16) -> Key {
    if code == 0 { Key::Parshape } else { Key::PenShape(code as u8 - 1) }
}

/// `\the\parshape` (lines) and e-TeX `\the\interlinepenalties n` & co (etex.ch).
fn shape_get(c: &mut dyn Ctx, code: u16) -> Val {
    if code == 0 {
        return match c.get(&Key::Parshape) {
            Val::Parshape(p) => Val::Int(p.len() as i32),
            _ => Val::Int(0),
        };
    }
    let n = scan::scan_int(c);
    let v = match c.get(&shape_key(code)) {
        Val::Ints(p) if !p.is_empty() && n >= 0 => {
            let k = (n as i64).min(p[0] as i64) as usize;
            p.get(k).copied().unwrap_or(0)
        }
        _ => 0,
    };
    Val::Int(v)
}

/// e-TeX `\parshapelength` 0, `\parshapeindent` 1, `\parshapedimen` 2.
fn parshape_dimen(c: &mut dyn Ctx, code: u16) -> Val {
    let n = scan::scan_int(c);
    let p = match c.get(&Key::Parshape) {
        Val::Parshape(p) => p,
        _ => return Val::Int(0),
    };
    if p.is_empty() || n <= 0 {
        return Val::Int(0);
    }
    // `q` 0 length, 1 indent (tex.web's mem[p+2k-q]).
    let (mut q, mut k) = (code as i64, n as i64);
    if q == 2 {
        q = k % 2;
        k = (k + q) / 2;
    }
    let k = k.min(p.len() as i64) as usize;
    let (indent, length) = p[k - 1];
    Val::Int(if q == 1 { indent } else { length })
}

// ---- helpers

fn globaldefs_key() -> Key {
    static I: OnceLock<u16> = OnceLock::new();
    Key::IntPar(*I.get_or_init(|| ip::of("globaldefs")))
}

fn relax_id() -> Option<PrimId> {
    static I: OnceLock<Option<PrimId>> = OnceLock::new();
    *I.get_or_init(|| registry().by_name("relax"))
}

/// A token as error messages print it.
pub fn show_tok(c: &dyn Ctx, t: &Tok) -> String {
    match t {
        Tok::Cs(n) => format!("\\{}", c.names().name(*n)),
        Tok::Active(ch) | Tok::Char { ch, .. } => ch.to_string(),
        Tok::Param(n) => format!("#{n}"),
        Tok::DontExpand => "\\notexpanded:".into(),
    }
}

fn esc(name: &str) -> String {
    format!("\\{name}")
}

/// The meaning of `t`: a character token means itself.
fn meaning_of(c: &mut dyn Ctx, t: &Tok) -> Meaning {
    match t {
        Tok::Char { cat, ch } => Meaning::Char { cat: *cat, ch: *ch },
        Tok::Param(_) | Tok::DontExpand => Meaning::Undefined,
        _ => c.meaning(t),
    }
}

fn is_spacer(c: &mut dyn Ctx, t: &Tok) -> bool {
    matches!(meaning_of(c, t), Meaning::Char { cat: 10, .. })
}

/// Keys whose changes are always global in TeX (font arrays, interaction).
fn always_global(k: &Key) -> bool {
    matches!(
        k,
        Key::FontDimen(..) | Key::HyphenChar(_) | Key::SkewChar(_) | Key::FontCode(..) | Key::Interaction
            | Key::Patterns(_) | Key::Exceptions(_) | Key::AfterAssignment
    )
}

/// A define with tex.web's `define`/`word_define` meaning of `global`
/// (already adjusted for `\globaldefs`).
fn def(c: &mut dyn Ctx, key: Key, val: Val, global: bool) {
    let g = global || always_global(&key);
    c.define(key, val, g);
}

/// `global` adjusted by `\globaldefs` (tex.web 1214).
pub fn effective_global(c: &mut dyn Ctx, global: bool) -> bool {
    let gd = c.int(&globaldefs_key());
    if gd > 0 {
        true
    } else if gd < 0 {
        false
    } else {
        global
    }
}

/// tex.web 1269: the token saved by `\afterassignment`, if any, goes back
/// into the input, and is forgotten.
pub fn after_assignment(c: &mut dyn Ctx) {
    let t = c.get(&Key::AfterAssignment).toks();
    if !t.is_empty() {
        c.push(t, TokKind::AfterAssignment);
        c.define(Key::AfterAssignment, Val::Toks(Arc::from(Vec::<Tok>::new())), true);
    }
}

/// Defines with `\globaldefs` and `\afterassignment` applied: for a whole
/// assignment made outside [`prefixed_command`] (e.g. a box assigned at its
/// group's end needs only [`effective_global`]).
pub fn assign(c: &mut dyn Ctx, key: Key, val: Val, global: bool) {
    let g = effective_global(c, global);
    def(c, key, val, g);
    after_assignment(c);
}

fn empty_toks() -> Toks {
    Arc::from(Vec::<Tok>::new())
}

// ---- entry points

fn cmd(c: &mut dyn Ctx, code: u16) {
    execute(c, Target::Mine(code), Tok::Cs(0), 0);
}

/// `\afterassignment` (tex.web 1268): not itself an assignment.
fn after_assignment_cmd(c: &mut dyn Ctx, _code: u16) {
    if let Some(t) = c.next() {
        c.define(Key::AfterAssignment, Val::Toks(Arc::from(vec![t])), true);
    }
}

/// A `Kind::Prefix` token was met (code 1 global, 2 long, 4 outer, 8 protected):
/// gathers further prefixes (tex.web 1211) and runs the assignment.
pub fn prefix(c: &mut dyn Ctx, code: u16) {
    let mut a = code as u8;
    loop {
        let Some(t) = scan::next_non_blank(c) else { return };
        let m = meaning_of(c, &t);
        if let Meaning::Prim(id) = m {
            let d = *registry().get(id);
            if let Kind::Prefix = d.kind {
                a |= d.code as u8;
                continue;
            }
        }
        match target_of(&m) {
            Some(target) => {
                execute(c, target, t, a);
            }
            None => {
                let s = show_tok(c, &t);
                c.back(t);
                error(
                    c,
                    &format!("You can't use a prefix with `{s}'"),
                    &[
                        "I'll pretend you didn't say \\long or \\outer or \\global.",
                        // e-TeX's help (with \protected)
                    ],
                );
            }
        }
        return;
    }
}

/// An assignment beginning with `first` (after prefixes were gathered: 1 global 2 long 4 outer 8 protected).
pub fn prefixed_command(c: &mut dyn Ctx, first: PrimId, prefixes: u8) {
    let d = *registry().get(first);
    let t = Tok::Cs(c.names().intern(d.name));
    match target_of(&Meaning::Prim(first)) {
        Some(target) => execute(c, target, t, prefixes),
        None => {
            let s = show_tok(c, &t);
            error(c, &format!("You can't use a prefix with `{s}'"), &["I'll pretend you didn't say \\long or \\outer or \\global."]);
        }
    }
}

/// A token meaning a font identifier (selects it) or a `\countdef`'d
/// register (assigns it), met in command position.
pub fn assign_meaning(c: &mut dyn Ctx, t: Tok, prefixes: u8) {
    let m = meaning_of(c, &t);
    if let Some(target) = target_of(&m) {
        execute(c, target, t, prefixes);
    }
}

#[derive(Clone, Copy)]
enum Target {
    Mine(u16),
    Prim(PrimId),
    Reg(u8, u16),
    Font(FontId),
}

/// Whether a meaning starts an assignment (tex.web's `cur_cmd > max_non_prefixed_command`).
fn target_of(m: &Meaning) -> Option<Target> {
    match m {
        Meaning::Font(f) => Some(Target::Font(*f)),
        Meaning::Reg(b, n) => Some(Target::Reg(*b, *n)),
        Meaning::Prim(id) => {
            let d = registry().get(*id);
            let ok = match d.kind {
                Kind::Quantity { .. } => true,
                Kind::Computed { set, .. } => set.is_some() || SHAPES.contains(&d.name),
                Kind::Command(_) => {
                    MINE.contains(&d.name) || DEFS.contains(&d.name) || OTHERS.contains(&d.name)
                }
                _ => false,
            };
            if !ok {
                return None;
            }
            if MINE.contains(&d.name) && matches!(d.kind, Kind::Command(_)) {
                Some(Target::Mine(d.code))
            } else {
                Some(Target::Prim(*id))
            }
        }
        _ => None,
    }
}

/// `prefixed_command` proper (tex.web 1211–1270), after the command token.
fn execute(c: &mut dyn Ctx, target: Target, t: Tok, a: u8) {
    let mut a = a;
    let is_def = match target {
        Target::Prim(id) => {
            let d = registry().get(id);
            DEFS.contains(&d.name) && matches!(d.kind, Kind::Command(_))
        }
        _ => false,
    };
    if !is_def && a & 0b1110 != 0 {
        let s = match target {
            Target::Mine(_) | Target::Reg(..) | Target::Font(_) if t == Tok::Cs(0) => String::from("this"),
            _ => show_tok(c, &t),
        };
        let msg = if c.etex() {
            format!("You can't use `\\long' or `\\outer' or `\\protected' with `{s}'")
        } else {
            format!("You can't use `\\long' or `\\outer' with `{s}'")
        };
        let help = if c.etex() {
            "I'll pretend you didn't say \\long or \\outer or \\protected here."
        } else {
            "I'll pretend you didn't say \\long or \\outer here."
        };
        error(c, &msg, &[help]);
        a &= 1;
    }
    // tex.web 1214: \globaldefs
    let gd = c.int(&globaldefs_key());
    if gd > 0 {
        a |= 1;
    } else if gd < 0 {
        a &= !1;
    }
    let g = a & 1 != 0;
    let done = match target {
        Target::Mine(code) => mine(c, code, g),
        Target::Font(f) => {
            def(c, Key::CurFont, Val::Font(f), g);
            true
        }
        Target::Reg(b, n) => assign_quantity(c, bank_level(b), bank_key(b, n), g),
        Target::Prim(id) => {
            let d: Def = *registry().get(id);
            match d.kind {
                Kind::Quantity { level, key } => {
                    let k = key(c, d.code);
                    assign_quantity(c, level, k, g)
                }
                Kind::Computed { .. } if SHAPES.contains(&d.name) => {
                    set_shape(c, d.code, g);
                    true
                }
                Kind::Computed { set: Some(f), .. } => {
                    f(c, d.code, Val::Void, g);
                    true
                }
                Kind::Command(f) => {
                    if DEFS.contains(&d.name) {
                        crate::macros::define_macro(c, d.code, a);
                    } else if d.name == "read" || d.name == "readline" {
                        read_to_cs(c, d.name == "readline", g);
                    } else if d.name == "font" {
                        crate::font::new_font(c, g);
                    } else {
                        f(c, d.code);
                    }
                    true
                }
                _ => true,
            }
        }
    };
    if done {
        after_assignment(c);
    }
}

// ---- the assignments

/// One of this module's commands; false: an error ended it (no `\afterassignment`).
fn mine(c: &mut dyn Ctx, code: u16, g: bool) -> bool {
    match code {
        LET | FUTURELET => {
            let_cmd(c, code == FUTURELET, g);
            true
        }
        CHARDEF..=TOKSDEF => {
            shorthand_def(c, code, g);
            true
        }
        ADVANCE..=DIVIDE => do_register_command(c, code, g),
        SETBOX => {
            set_box(c, g);
            true
        }
        _ => true,
    }
}

fn r_key(t: &Tok) -> Key {
    match t {
        Tok::Active(ch) => Key::Active(*ch),
        Tok::Cs(n) => Key::Cs(*n),
        _ => Key::Cs(0),
    }
}

/// `get_r_token` (tex.web 1215): the name to be defined.
pub fn get_r_token(c: &mut dyn Ctx) -> Tok {
    loop {
        let t = match c.next() {
            Some(t) => t,
            None => return Tok::Cs(c.names().intern("inaccessible ")),
        };
        if t == (Tok::Char { cat: 10, ch: ' ' }) {
            continue;
        }
        match t {
            Tok::Cs(_) | Tok::Active(_) => return t,
            _ => {
                error(
                    c,
                    "Missing control sequence inserted",
                    &[
                        "Please don't say `\\def cs{...}', say `\\def\\cs{...}'.",
                        "I've inserted an inaccessible control sequence so that your",
                        "definition will be completed without mixing me up too badly.",
                        "You can recover graciously from this error, if you're",
                        "careful; see exercise 27.2 in The TeXbook.",
                    ],
                );
                c.back(t);
                return Tok::Cs(c.names().intern("inaccessible "));
            }
        }
    }
}

/// `\let` / `\futurelet` (tex.web 1221).
fn let_cmd(c: &mut dyn Ctx, future: bool, g: bool) {
    let p = get_r_token(c);
    let t = if !future {
        let mut t = loop {
            match c.next() {
                Some(t) if is_spacer(c, &t) => continue,
                Some(t) => break t,
                None => return,
            }
        };
        if t == (Tok::Char { cat: 12, ch: '=' }) {
            match c.next() {
                Some(u) => t = u,
                None => return,
            }
            if is_spacer(c, &t) {
                match c.next() {
                    Some(u) => t = u,
                    None => return,
                }
            }
        }
        t
    } else {
        let Some(q) = c.next() else { return };
        let Some(t) = c.next() else {
            c.back(q);
            return;
        };
        c.back(t);
        c.back(q);
        t
    };
    let m = meaning_of(c, &t);
    def(c, r_key(&p), Val::Meaning(m), g);
}

/// `\chardef` `\mathchardef` `\countdef` … (tex.web 1224).
fn shorthand_def(c: &mut dyn Ctx, code: u16, g: bool) {
    let p = get_r_token(c);
    let k = r_key(&p);
    // `define(p, relax, 256)` first: `\chardef\x=\x` sees \relax.
    if let Some(r) = relax_id() {
        def(c, k.clone(), Val::Meaning(Meaning::Prim(r)), g);
    }
    scan::scan_optional_equals(c);
    let m = match code {
        CHARDEF => Meaning::CharGiven(scan::scan_char_num(c) as u32),
        MATHCHARDEF => Meaning::MathCharGiven(scan::scan_fifteen_bit_int(c)),
        _ => Meaning::Reg((code - COUNTDEF) as u8, scan::scan_register_num(c)),
    };
    def(c, k, Val::Meaning(m), g);
}

/// The largest legal value of a code table entry, and whether negatives are illegal.
fn code_range(k: &Key) -> Option<(i32, bool)> {
    match k {
        Key::Cat(_) => Some((15, true)),
        Key::Lc(_) | Key::Uc(_) => Some((255, true)),
        Key::Sf(_) => Some((0o77777, true)),
        Key::MathCode(_) => Some((0o100000, true)),
        Key::DelCode(_) => Some((0o77777777, false)),
        _ => None,
    }
}

/// `=` and the value of a quantity at `level`, assigned to `key` (tex.web
/// 1224–1232). False: an error ended it.
fn assign_quantity(c: &mut dyn Ctx, level: Level, key: Key, g: bool) -> bool {
    match level {
        Level::Tok => assign_toks(c, key, g),
        Level::Ident => {
            scan::scan_optional_equals(c);
            let f = scan::scan_font_ident(c);
            def(c, key, Val::Font(f), g);
        }
        Level::Int => {
            scan::scan_optional_equals(c);
            let mut v = scan::scan_int(c);
            if let Some((n, nonneg)) = code_range(&key) {
                if (v < 0 && nonneg) || v > n {
                    let msg = if nonneg {
                        format!("Invalid code ({v}), should be in the range 0..{n}")
                    } else {
                        format!("Invalid code ({v}), should be at most {n}")
                    };
                    error(c, &msg, &["I'm going to use 0 instead of that illegal code value."]);
                    v = 0;
                }
            }
            def(c, key, Val::Int(v), g);
        }
        Level::Dimen => {
            scan::scan_optional_equals(c);
            let v = scan::scan_normal_dimen(c);
            def(c, key, Val::Int(v), g);
        }
        Level::Glue | Level::Mu => {
            scan::scan_optional_equals(c);
            let v = scan::scan_glue(c, level);
            def(c, key, Val::Glue(v), g);
        }
    }
    true
}

/// A token list assignment (tex.web 1226–1227).
fn assign_toks(c: &mut dyn Ctx, key: Key, g: bool) {
    scan::scan_optional_equals(c);
    let Some(t) = scan::next_non_blank(c) else { return };
    let is_left_brace = matches!(meaning_of(c, &t), Meaning::Char { cat: 1, .. });
    if !is_left_brace {
        // the right-hand side a token parameter or register?
        let src = match meaning_of(c, &t) {
            Meaning::Reg(4, n) => Some(Key::Toks(n)),
            Meaning::Prim(id) => {
                let d = *registry().get(id);
                match d.kind {
                    Kind::Quantity { level: Level::Tok, key } => Some(key(c, d.code)),
                    _ => None,
                }
            }
            _ => None,
        };
        if let Some(src) = src {
            let v = c.get(&src).toks();
            def(c, key, Val::Toks(v), g);
            return;
        }
    }
    c.back(t);
    let (_, body) = scan::scan_toks(c, false, false);
    if body.is_empty() {
        def(c, key, Val::Toks(empty_toks()), g);
        return;
    }
    let body = if key == Key::TokPar(tp::of("output")) {
        // enclose \output in braces
        let mut v = Vec::with_capacity(body.len() + 2);
        v.push(Tok::Char { cat: 1, ch: '{' });
        v.extend(body.iter().copied());
        v.push(Tok::Char { cat: 2, ch: '}' });
        Arc::from(v)
    } else {
        body
    };
    def(c, key, Val::Toks(body), g);
}

/// Whether an arithmetic command may name this quantity (tex.web 1237:
/// registers and integer/dimen/glue/muglue parameters).
fn arith_target(c: &mut dyn Ctx, t: &Tok) -> Option<(Level, Key)> {
    match meaning_of(c, t) {
        Meaning::Reg(b, n) if b < 4 => Some((bank_level(b), bank_key(b, n))),
        Meaning::Prim(id) => {
            let d = *registry().get(id);
            let Kind::Quantity { level, key } = d.kind else { return None };
            let ok = matches!(d.name, "count" | "dimen" | "skip" | "muskip")
                || ip::NAMES.contains(&d.name)
                || dp::NAMES.contains(&d.name)
                || gp::NAMES.contains(&d.name)
                || mp::NAMES.contains(&d.name);
            if !ok || level > Level::Mu {
                return None;
            }
            Some((level, key(c, d.code)))
        }
        _ => None,
    }
}

/// `\advance` `\multiply` `\divide` (tex.web 1236–1240).
fn do_register_command(c: &mut dyn Ctx, q: u16, g: bool) -> bool {
    let what = match q {
        ADVANCE => "advance",
        MULTIPLY => "multiply",
        _ => "divide",
    };
    let Some(t) = c.next_x() else { return false };
    let Some((level, key)) = arith_target(c, &t) else {
        let s = show_tok(c, &t);
        error(
            c,
            &format!("You can't use `{s}' after {}", esc(what)),
            &["I'm forgetting what you said and not changing anything."],
        );
        return false;
    };
    scan::scan_keyword(c, "by");
    let old = c.get(&key);
    let new: Option<Val> = match (q, level) {
        (ADVANCE, Level::Int) => Some(Val::Int(old.int().wrapping_add(scan::scan_int(c)))),
        (ADVANCE, Level::Dimen) => Some(Val::Int(old.int().wrapping_add(scan::scan_normal_dimen(c)))),
        (ADVANCE, _) => {
            let v = scan::scan_glue(c, level);
            Some(Val::Glue(add_glue(v, old.glue())))
        }
        (_, _) => {
            let n = scan::scan_int(c);
            match (q, level) {
                (MULTIPLY, Level::Int) => arith::mult_integers(old.int(), n).map(Val::Int),
                (MULTIPLY, Level::Dimen) => arith::nx_plus_y(old.int(), n, 0).map(Val::Int),
                (MULTIPLY, _) => {
                    let s = old.glue();
                    (|| {
                        Some(Val::Glue(Glue {
                            width: arith::nx_plus_y(s.width, n, 0)?,
                            stretch: arith::nx_plus_y(s.stretch, n, 0)?,
                            shrink: arith::nx_plus_y(s.shrink, n, 0)?,
                            ..s
                        }))
                    })()
                }
                (_, Level::Int | Level::Dimen) => arith::x_over_n_checked(old.int(), n).map(|(q, _)| Val::Int(q)),
                (_, _) => {
                    let s = old.glue();
                    (|| {
                        Some(Val::Glue(Glue {
                            width: arith::x_over_n_checked(s.width, n)?.0,
                            stretch: arith::x_over_n_checked(s.stretch, n)?.0,
                            shrink: arith::x_over_n_checked(s.shrink, n)?.0,
                            ..s
                        }))
                    })()
                }
            }
        }
    };
    match new {
        Some(v) => {
            def(c, key, v, g);
            true
        }
        None => {
            error(
                c,
                "Arithmetic overflow",
                &[
                    "I can't carry out that multiplication or division,",
                    "since the result is out of range.",
                ],
            );
            false
        }
    }
}

/// tex.web 1239: `q` (the scanned glue) plus `r` (the old value).
fn add_glue(q: Glue, r: Glue) -> Glue {
    let mut q = q;
    q.width = q.width.wrapping_add(r.width);
    if q.stretch == 0 {
        q.stretch_order = 0;
    }
    if q.stretch_order == r.stretch_order {
        q.stretch = q.stretch.wrapping_add(r.stretch);
    } else if q.stretch_order < r.stretch_order && r.stretch != 0 {
        q.stretch = r.stretch;
        q.stretch_order = r.stretch_order;
    }
    if q.shrink == 0 {
        q.shrink_order = 0;
    }
    if q.shrink_order == r.shrink_order {
        q.shrink = q.shrink.wrapping_add(r.shrink);
    } else if q.shrink_order < r.shrink_order && r.shrink != 0 {
        q.shrink = r.shrink;
        q.shrink_order = r.shrink_order;
    }
    q
}

/// `\setbox` (tex.web 1241, 1084 `scan_box`).
fn set_box(c: &mut dyn Ctx, g: bool) {
    let n = scan::scan_register_num(c);
    scan::scan_optional_equals(c);
    let Some(t) = scan::next_non_blank(c) else { return };
    let is_box = match meaning_of(c, &t) {
        Meaning::Prim(id) => {
            matches!(registry().get(id).name, "box" | "copy" | "lastbox" | "vsplit" | "hbox" | "vbox" | "vtop")
        }
        _ => false,
    };
    c.back(t);
    if is_box {
        // `begin_box` reads the box command token itself.
        crate::boxes::begin_box(c, crate::boxes::BoxContext::SetBox(n, g));
    } else {
        error(
            c,
            "A <box> was supposed to be here",
            &[
                "I was expecting to see \\hbox or \\vbox or \\copy or \\box or",
                "something like that. So you might find something missing in",
                "your output. But keep trying; you can fix this later.",
            ],
        );
    }
}

/// `\parshape` and e-TeX's penalty arrays (tex.web 1248, etex.ch).
fn set_shape(c: &mut dyn Ctx, code: u16, g: bool) {
    scan::scan_optional_equals(c);
    let n = scan::scan_int(c);
    if code == 0 {
        let mut v = Vec::new();
        for _ in 0..n.max(0) {
            let indent = scan::scan_normal_dimen(c);
            let length = scan::scan_normal_dimen(c);
            v.push((indent, length));
        }
        def(c, Key::Parshape, Val::Parshape(Arc::from(v)), g);
    } else {
        let mut v = Vec::new();
        if n > 0 {
            v.push(n);
            for _ in 0..n {
                v.push(scan::scan_int(c));
            }
        }
        def(c, shape_key(code), Val::Ints(Arc::from(v)), g);
    }
}

/// `\read n to \cs` / e-TeX `\readline` (tex.web 1225): the line's tokens
/// (from `io.rs`) become a parameterless macro.
fn read_to_cs(c: &mut dyn Ctx, readline: bool, g: bool) {
    let n = scan::scan_int(c);
    if !scan::scan_keyword(c, "to") {
        error(
            c,
            "Missing `to' inserted",
            &["You should have said `\\read<number> to \\cs'.", "I'm going to look for the \\cs now."],
        );
    }
    let p = get_r_token(c);
    let body = crate::io::read_toks(c, n, readline);
    let m = Macro { params: empty_toks(), body, brace_end: false, long: false, outer: false, protected: false };
    def(c, r_key(&p), Val::Meaning(Meaning::Macro(Arc::new(m))), g);
}
