//! D2: `\output` applied at a fire (box255, marks, inserts), `\shipout`,
//! whatsits expanded at shipout (`\write` text → stream/written file).
//!
//! A fire (`Op::Fire`, made by `page.rs`) calls [`fire`]: it defines box255,
//! the insertion boxes, the marks and `\outputpenalty`, then either pushes
//! the `\output` tokens (`TokKind::Output`) inside a new inner vertical list
//! and an `Output` group, or ships box255 itself (empty `\output`, or too many
//! dead cycles). The routine's closing `}` reaches [`output_group_end`]
//! through `groups::right_brace`; the routine's list is emitted into the
//! enclosing (main vertical) list, ahead of the page builder's contributions.
use std::sync::Arc;

use crate::ctx::Ctx;
use crate::env::{GroupKind, Mode};
use crate::item::{BoxNode, Item};
use crate::prim::{dp, ip, tp, Kind, Registry};
use crate::term::{Emit, TokKind};
use crate::tok::{Tok, Toks};
use crate::val::{Key, Val};

pub fn register(r: &mut Registry) {
    r.add("shipout", 0, Kind::Command(shipout_cmd), 0);
}

/// A shipped page: its box, `\count0..9`, and the PDF/DVI parameters read at shipout.
#[derive(Clone, Debug, PartialEq)]
pub struct Shipped { pub b: Arc<crate::item::BoxNode>, pub counts: [i32; 10], pub params: Arc<[(crate::val::Key, crate::val::Val)]> }

/// `output_active` (global while an output routine runs).
pub const ACTIVE: Key = Key::Other(crate::prim::tag::OUTPUT, 0);

pub fn output_active(c: &mut dyn Ctx) -> bool {
    c.int(&ACTIVE) != 0
}

const IGNORE_DEPTH: i32 = -65_536_000;
const MAX_DIMEN: i64 = 0o7777777777;

/// What the page builder hands to a fire (tex.web 1012–1018 inputs).
#[derive(Clone, Debug, PartialEq)]
pub struct FireIn {
    /// The page, already packaged to `\vsize` (box255).
    pub box255: Arc<BoxNode>,
    /// Insertion boxes `\box n` as they are after the page's insertions.
    pub inserts: Vec<(u8, Option<Arc<BoxNode>>)>,
    /// Marks per class: (class, top, first, bot).
    pub marks: Vec<(u16, Toks, Toks, Toks)>,
    /// `\outputpenalty`: the best break's penalty (10000 if not a penalty).
    pub penalty: i32,
}

/// Applies `\output` at a fire (tex.web 1012, 1017, 1023–1025).
pub fn fire(c: &mut dyn Ctx, f: &FireIn) {
    c.define(Key::IntPar(ip::of("outputpenalty")), Val::Int(f.penalty), true);
    for (class, top, first, bot) in &f.marks {
        c.define(Key::Mark(*class, 0), Val::Toks(top.clone()), true);
        c.define(Key::Mark(*class, 1), Val::Toks(first.clone()), true);
        c.define(Key::Mark(*class, 2), Val::Toks(bot.clone()), true);
    }
    for (n, b) in &f.inserts {
        c.define(Key::Box(*n as u16), Val::Box(b.clone()), true);
    }
    c.define(Key::Box(255), Val::Box(Some(f.box255.clone())), true);

    let routine = c.get(&Key::TokPar(tp::of("output"))).toks();
    if !routine.is_empty() {
        let dead = c.int(&Key::Page(8));
        let max = c.int(&Key::IntPar(ip::of("maxdeadcycles")));
        if dead >= max {
            crate::diag::error(
                c,
                &format!("Output loop---{dead} consecutive dead cycles"),
                &[
                    "I've concluded that your \\output is awry; it never does a",
                    "\\shipout, so I'm shipping \\box255 out myself. Next time",
                    "increase \\maxdeadcycles if you want me to be more patient!",
                ],
            );
        } else {
            begin_output(c, routine, dead);
            return;
        }
    }
    // the default output routine (tex.web 1023)
    if let Val::Box(Some(b)) = c.get(&Key::Box(255)) {
        ship_out(c, b);
    }
    c.define(Key::Box(255), Val::Box(None), true);
}

fn begin_output(c: &mut dyn Ctx, routine: Toks, dead: i32) {
    c.define(ACTIVE, Val::Int(1), true);
    c.define(Key::Page(8), Val::Int(dead + 1), true);
    c.open_list(Mode::Vertical { inner: true });
    c.define(Key::PrevDepth, Val::Int(IGNORE_DEPTH), false);
    // `\output` text is kept braced (tex.web 1226); wrap it if it is not.
    let braced = matches!(routine.first(), Some(Tok::Char { cat: 1, .. }))
        && matches!(routine.last(), Some(Tok::Char { cat: 2, .. }));
    let toks: Toks = if braced {
        routine
    } else {
        let mut v = Vec::with_capacity(routine.len() + 2);
        v.push(Tok::Char { cat: 1, ch: '{' });
        v.extend(routine.iter().copied());
        v.push(Tok::Char { cat: 2, ch: '}' });
        Arc::from(v)
    };
    c.push(toks, TokKind::Output);
    c.open_group(GroupKind::Output);
    normal_paragraph(c);
    crate::scan::scan_left_brace(c);
}

/// `normal_paragraph` (tex.web 1070), local to the current group.
pub fn normal_paragraph(c: &mut dyn Ctx) {
    let looseness = Key::IntPar(ip::of("looseness"));
    if c.int(&looseness) != 0 {
        c.define(looseness, Val::Int(0), false);
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

/// The output routine's `}` (tex.web 1100): the routine's list goes to the
/// enclosing list ahead of the contributions; box255 must be void.
pub fn output_group_end(c: &mut dyn Ctx) {
    if !matches!(c.reading(), Some(TokKind::Output) | Some(TokKind::Backed)) {
        crate::diag::error(
            c,
            "Unbalanced output routine",
            &["Your sneaky output routine has problematic {'s and/or }'s.", "I can't handle that very well; good luck."],
        );
    }
    crate::list::end_graf(c);
    c.close_group();
    c.define(ACTIVE, Val::Int(0), true);
    c.define(Key::Page(9), Val::Int(0), true);
    if let Val::Box(Some(_)) = c.get(&Key::Box(255)) {
        crate::diag::error(
            c,
            "Output routine didn't use all of \\box255",
            &[
                "Your \\output commands should empty \\box255,",
                "e.g., by saying `\\shipout\\box255'.",
                "Proceed; I'll discard its present contents.",
            ],
        );
        c.define(Key::Box(255), Val::Box(None), true);
    }
    let items = c.close_list();
    for it in items {
        c.item(it);
    }
}

/// `\shipout`: the box is scanned by `boxes.rs`, whose `box_end` with
/// `BoxContext::Ship` calls [`ship_out`].
fn shipout_cmd(c: &mut dyn Ctx, _code: u16) {
    crate::boxes::begin_box(c, crate::boxes::BoxContext::Ship);
}

/// `ship_out` (tex.web 638–640): performs the page's whatsits (open, write,
/// close: `\write` texts expanded now), then emits the page.
pub fn ship_out(c: &mut dyn Ctx, b: Arc<BoxNode>) {
    let mut counts = [0i32; 10];
    for (i, n) in counts.iter_mut().enumerate() {
        *n = c.int(&Key::Count(i as u16));
    }
    let mut last = 0;
    for (i, n) in counts.iter().enumerate() {
        if *n != 0 {
            last = i;
        }
    }
    let mut label = format!("[{}", counts[0]);
    for n in &counts[1..=last] {
        label.push_str(&format!(".{n}"));
    }
    c.emit(Emit::Write { stream: crate::io::MESSAGE, text: Arc::from(label) });

    let hoff = c.int(&Key::DimenPar(dp::of("hoffset"))) as i64;
    let voff = c.int(&Key::DimenPar(dp::of("voffset"))) as i64;
    let (w, h, d) = (b.width as i64, b.height as i64, b.depth as i64);
    if h > MAX_DIMEN || d > MAX_DIMEN || h + d + voff > MAX_DIMEN || w + hoff > MAX_DIMEN {
        crate::diag::error(
            c,
            "Huge page cannot be shipped out",
            &["The page just created is too large to be shipped out,", "so I can't really ship it out."],
        );
    } else {
        let mut st = crate::io::OpenState::default();
        walk(c, &b.list, &mut st);
        let params = read_params(c);
        c.emit(Emit::Ship(Arc::new(Shipped { b, counts, params })));
    }
    c.emit(Emit::Write { stream: crate::io::MESSAGE, text: Arc::from("]") });
    c.define(Key::Page(8), Val::Int(0), true);
}

/// Whatsits in output order (nested boxes depth first; leader boxes not repeated).
fn walk(c: &mut dyn Ctx, list: &[Item], st: &mut crate::io::OpenState) {
    for it in list {
        match it {
            Item::Box(b) => walk(c, &b.list, st),
            Item::Whatsit(w) => crate::io::out_what(c, w, st),
            _ => {}
        }
        if c.halted() {
            return;
        }
    }
}

/// The parameters the back ends read at shipout.
fn read_params(c: &mut dyn Ctx) -> Arc<[(Key, Val)]> {
    let mut keys = vec![
        Key::IntPar(ip::of("mag")),
        Key::DimenPar(dp::of("hoffset")),
        Key::DimenPar(dp::of("voffset")),
    ];
    if c.pdf() {
        for n in [
            "pdfoutput", "pdfcompresslevel", "pdfdecimaldigits", "pdfobjcompresslevel",
            "pdfgentounicode", "pdfdraftmode", "pdfpkresolution", "pdfimageresolution",
            "pdfmajorversion", "pdfminorversion",
        ] {
            keys.push(Key::IntPar(ip::of(n)));
        }
        for n in ["pdfhorigin", "pdfvorigin", "pdfpagewidth", "pdfpageheight"] {
            keys.push(Key::DimenPar(dp::of(n)));
        }
        for n in ["pdfpagesattr", "pdfpageattr", "pdfpageresources", "pdfpkmode"] {
            keys.push(Key::TokPar(tp::of(n)));
        }
    }
    let v: Vec<(Key, Val)> = keys.into_iter().map(|k| {
        let v = c.get(&k);
        (k, v)
    }).collect();
    Arc::from(v)
}
