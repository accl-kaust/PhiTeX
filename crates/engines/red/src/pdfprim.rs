//! D3: pdfTeX primitives (`\pdfobj` `\pdfrefobj` `\pdfannot` `\pdfstartlink`
//! `\pdfendlink` `\pdfdest` `\pdfoutline` `\pdfliteral` `\pdfxform` `\pdfximage`
//! `\pdfrefx*` `\pdfcatalog` `\pdfinfo` `\pdfnames` `\pdftrailer` `\pdfsavepos`
//! `\pdflastx/ypos` `\pdfcolorstack` `\pdfsetmatrix` `\pdfsave/restore` …).
//!
//! Nothing here is machine state. Every id-bearing thing (`\pdfobj`,
//! `\pdfxform`, `\pdfximage`, `\pdfannot`, `\pdfstartlink`) takes the next
//! abstract id from the counter `Key::PdfLast(L_IDS)` (a global value); E1 maps
//! ids to object numbers deterministically at assembly. An object's content is
//! the global value `Key::Other(tag::PDF, id)` (`Val::Data(PdfObj)`), and the
//! whatsits that reference one carry it too, so a shipped page is
//! self-contained. Job-level material (outlines, `\pdfcatalog`, `\pdfinfo`,
//! `\pdfnames`, `\pdftrailer`, map lines, `\pdfglyphtounicode`, immediate
//! objects) lives under the keys described at `K_*`; `job(&Env)` gathers it
//! from the final env for E1.
use std::any::Any;
use std::sync::Arc;

use crate::ctx::Ctx;
use crate::diag::error;
use crate::env::{Env, Mode};
use crate::item::{BoxNode, Item, Whatsit, RUNNING};
use crate::prim::{ip, registry, tag, Kind, Level, PrimId, Registry};
use crate::scan::{scan_font_ident, scan_int, scan_keyword, scan_normal_dimen, scan_register_num, scan_toks};
use crate::term::TokKind;
use crate::tok::{Tok, Toks};
use crate::val::{Datum, Key, Meaning, Val};

// ---- `Key::PdfLast` slots
pub const L_IDS: u8 = 0;
pub const L_OBJ: u8 = 1;
pub const L_XFORM: u8 = 2;
pub const L_XIMAGE: u8 = 3;
pub const L_XIMAGE_PAGES: u8 = 4;
pub const L_ANNOT: u8 = 5;
pub const L_LINK: u8 = 6;
/// Set by the shipout of a page holding `\pdfsavepos` (D2/E1).
pub const L_XPOS: u8 = 7;
pub const L_YPOS: u8 = 8;
/// Colour stacks made by `\pdfcolorstackinit` (stack 0 is predefined).
pub const L_COLORSTACKS: u8 = 9;
pub const L_XIMAGE_DEPTH: u8 = 10;
pub const L_OUTLINES: u8 = 11;
pub const L_G2U: u8 = 12;
pub const L_MAPS: u8 = 13;
pub const L_TIMER: u8 = 14;

// ---- `Key::Other(tag::PDF, n)` ranges (object ids are below K_NOLIG)
/// `| font`: `\pdfnoligatures` given (`Val::Int(1)`).
pub const K_NOLIG: u32 = 0x1000_0000;
/// `| page`: the page's object number (answered by E1/the fixpoint loop-back).
pub const K_PAGEREF: u32 = 0x2000_0000;
/// `| n`: colour stack n's init (`Val::Data(StackInit)`).
pub const K_CSINIT: u32 = 0x3000_0000;
/// `| i`: the i-th outline (from 1).
pub const K_OUTLINE: u32 = 0x4000_0000;
/// `| i`: the i-th `\pdfglyphtounicode` (from 1).
pub const K_G2U: u32 = 0x5000_0000;
/// `| i`: the i-th `\pdfmapfile`/`\pdfmapline` (from 1).
pub const K_MAP: u32 = 0x6000_0000;
/// Job texts (`Val::Name`): info, catalog, names, trailer; openaction (`Val::Data(Action)`).
pub const K_INFO: u32 = 0x7000_0001;
pub const K_CATALOG: u32 = 0x7000_0002;
pub const K_NAMES: u32 = 0x7000_0003;
pub const K_TRAILER: u32 = 0x7000_0004;
pub const K_OPENACTION: u32 = 0x7000_0005;

/// A destination or thread identifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DestId {
    Num(i32),
    Name(Arc<str>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GotoTarget {
    Num(i32),
    Name(Arc<str>),
    /// `page n {fit spec}`.
    Page { page: i32, spec: Arc<str> },
}

/// An action spec (pdftex `scan_action`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    User(Arc<str>),
    Goto { file: Option<Arc<str>>, target: GotoTarget, new_window: Option<bool> },
    Thread { file: Option<Arc<str>>, id: DestId, new_window: Option<bool> },
}

/// A destination's view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DestKind {
    Xyz { zoom: Option<i32> },
    Fit,
    FitH,
    FitV,
    FitB,
    FitBH,
    FitBV,
    /// The whatsit's (w, h, d) are the rule spec, `RUNNING` where not given.
    FitR,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObjData {
    Text(Arc<str>),
    File { name: Arc<str>, bytes: Arc<[u8]> },
}

/// An object made by `\pdfobj`/`\pdfxform`/`\pdfximage` (or an id reserved).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PdfObj {
    /// `reserveobjnum`: an id, no content yet.
    Reserved,
    Obj { immediate: bool, stream: bool, attr: Option<Arc<str>>, data: ObjData },
    Xform { immediate: bool, attr: Option<Arc<str>>, resources: Option<Arc<str>>, b: Arc<BoxNode> },
    Image(Arc<Image>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub immediate: bool,
    pub name: Arc<str>,
    pub bytes: Arc<[u8]>,
    pub page: i32,
    pub named: Option<Arc<str>>,
    /// 1 media 2 crop 3 bleed 4 trim 5 art.
    pub pagebox: u8,
    pub colorspace: i32,
    pub attr: Option<Arc<str>>,
    pub width: i32,
    pub height: i32,
    pub depth: i32,
    pub pages: i32,
    pub colordepth: i32,
}

/// A colour stack's initial state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackInit {
    pub page: bool,
    pub direct: bool,
    pub literal: Arc<str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackOp {
    Set,
    Push,
    Pop,
    Current,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outline {
    pub attr: Option<Arc<str>>,
    pub action: Action,
    pub count: i32,
    pub title: Arc<str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapEntry {
    /// true `\pdfmapfile`, false `\pdfmapline`.
    pub file: bool,
    pub text: Arc<str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlyphToUnicode {
    pub glyph: Arc<str>,
    pub unicode: Arc<str>,
}

/// pdfTeX's `\pdfliteral` modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiteralMode {
    SetOrigin,
    Page,
    Direct,
}

/// A pdfTeX whatsit (`Whatsit::Pdf`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PdfWhat {
    /// `text` was expanded and made a string at the command; `ship`: the
    /// `shipout` form, tokens expanded at shipout instead (by D2, like `\write`).
    Literal { mode: LiteralMode, text: Arc<str>, ship: Option<Toks> },
    RefObj { id: u32, obj: Arc<PdfObj> },
    RefXform { id: u32, width: i32, height: i32, depth: i32, form: Arc<PdfObj> },
    RefXimage { id: u32, width: i32, height: i32, depth: i32, image: Arc<PdfObj> },
    /// Dimensions `RUNNING` where not given (taken from the enclosing box at shipout).
    Annot { id: u32, width: i32, height: i32, depth: i32, data: Arc<str> },
    StartLink { id: u32, width: i32, height: i32, depth: i32, attr: Option<Arc<str>>, action: Arc<Action> },
    EndLink,
    Dest { id: DestId, kind: DestKind, width: i32, height: i32, depth: i32 },
    /// `start` false `\pdfthread`, true `\pdfstartthread`.
    Thread { start: bool, id: DestId, attr: Option<Arc<str>>, width: i32, height: i32, depth: i32 },
    EndThread,
    SavePos,
    ColorStack { stack: u32, op: StackOp, text: Arc<str>, init: Arc<StackInit> },
    SetMatrix(Arc<str>),
    Save,
    Restore,
}

impl PdfWhat {
    /// Size of a whatsit that takes room in a list (`\pdfrefxform`,
    /// `\pdfrefximage`), for `hpack`/`vpack` and shipout: (w, h, d).
    pub fn dims(&self) -> Option<(i32, i32, i32)> {
        match self {
            PdfWhat::RefXform { width, height, depth, .. } | PdfWhat::RefXimage { width, height, depth, .. } => {
                Some((*width, *height, *depth))
            }
            _ => None,
        }
    }
}

macro_rules! datum {
    ($($t:ty),*) => {$(
        impl Datum for $t {
            fn same(&self, o: &dyn Datum) -> bool {
                o.as_any().downcast_ref::<$t>().is_some_and(|x| x == self)
            }
            fn as_any(&self) -> &dyn Any {
                self
            }
        }
    )*};
}
datum!(PdfObj, StackInit, Outline, MapEntry, Action, GlyphToUnicode);

fn data<T: Clone + 'static>(v: &Val) -> Option<T> {
    match v {
        Val::Data(d) => d.as_any().downcast_ref::<T>().cloned(),
        _ => None,
    }
}

// ---- codes
const LITERAL: u16 = 0;
const OBJ: u16 = 1;
const REFOBJ: u16 = 2;
const XFORM: u16 = 3;
const REFXFORM: u16 = 4;
const XIMAGE: u16 = 5;
const REFXIMAGE: u16 = 6;
const ANNOT: u16 = 7;
const STARTLINK: u16 = 8;
const ENDLINK: u16 = 9;
const DEST: u16 = 10;
const OUTLINE: u16 = 11;
const THREAD: u16 = 12;
const STARTTHREAD: u16 = 13;
const ENDTHREAD: u16 = 14;
const INFO: u16 = 15;
const CATALOG: u16 = 16;
const NAMES: u16 = 17;
const TRAILER: u16 = 18;
const SAVEPOS: u16 = 19;
const COLORSTACK: u16 = 20;
const SETMATRIX: u16 = 21;
const SAVE: u16 = 22;
const RESTORE: u16 = 23;
const SETRANDOMSEED: u16 = 24;
const RESETTIMER: u16 = 25;
const G2U: u16 = 26;
const MAPFILE: u16 = 27;
const MAPLINE: u16 = 28;
const NOLIG: u16 = 29;
const QUITVMODE: u16 = 30;

const X_UNIFORM: u16 = 0;
const X_NORMAL: u16 = 1;
const X_CSINIT: u16 = 2;
const X_PAGEREF: u16 = 3;
const X_PRIMITIVE: u16 = 4;

const C_RANDOMSEED: u16 = 100;
const C_ELAPSED: u16 = 101;

pub fn register(r: &mut Registry) {
    let cmds: &[(&'static str, u16)] = &[
        ("pdfliteral", LITERAL),
        ("pdfobj", OBJ),
        ("pdfrefobj", REFOBJ),
        ("pdfxform", XFORM),
        ("pdfrefxform", REFXFORM),
        ("pdfximage", XIMAGE),
        ("pdfrefximage", REFXIMAGE),
        ("pdfannot", ANNOT),
        ("pdfstartlink", STARTLINK),
        ("pdfendlink", ENDLINK),
        ("pdfdest", DEST),
        ("pdfoutline", OUTLINE),
        ("pdfthread", THREAD),
        ("pdfstartthread", STARTTHREAD),
        ("pdfendthread", ENDTHREAD),
        ("pdfinfo", INFO),
        ("pdfcatalog", CATALOG),
        ("pdfnames", NAMES),
        ("pdftrailer", TRAILER),
        ("pdfsavepos", SAVEPOS),
        ("pdfcolorstack", COLORSTACK),
        ("pdfsetmatrix", SETMATRIX),
        ("pdfsave", SAVE),
        ("pdfrestore", RESTORE),
        ("pdfsetrandomseed", SETRANDOMSEED),
        ("pdfresettimer", RESETTIMER),
        // `\pdfglyphtounicode` `\pdfmapfile` `\pdfmapline`: pdffont.rs's;
        // `\pdfnoligatures`: font.rs's.
        ("quitvmode", QUITVMODE),
    ];
    for &(n, code) in cmds {
        r.add(n, code, Kind::Command(command), 2);
    }
    let xs: &[(&'static str, u16)] = &[
        ("pdfuniformdeviate", X_UNIFORM),
        ("pdfnormaldeviate", X_NORMAL),
        ("pdfcolorstackinit", X_CSINIT),
        ("pdfpageref", X_PAGEREF),
        ("pdfprimitive", X_PRIMITIVE),
    ];
    for &(n, code) in xs {
        r.add(n, code, Kind::Expand(expand), 2);
    }
    let lasts: &[(&'static str, u16)] = &[
        ("pdflastobj", L_OBJ as u16),
        ("pdflastxform", L_XFORM as u16),
        ("pdflastximage", L_XIMAGE as u16),
        ("pdflastximagepages", L_XIMAGE_PAGES as u16),
        ("pdflastximagecolordepth", L_XIMAGE_DEPTH as u16),
        ("pdflastannot", L_ANNOT as u16),
        ("pdflastlink", L_LINK as u16),
        ("pdflastxpos", L_XPOS as u16),
        ("pdflastypos", L_YPOS as u16),
        ("pdfrandomseed", C_RANDOMSEED),
        ("pdfelapsedtime", C_ELAPSED),
    ];
    for &(n, code) in lasts {
        r.add(n, code, Kind::Computed { level: Level::Int, get: last_item, set: None }, 2);
    }
}

/// Is `id` one of the primitives `\immediate` applies to here
/// (`\pdfobj` `\pdfxform` `\pdfximage`)?
pub fn immediate_able(id: PrimId) -> bool {
    let d = registry().get(id);
    matches!(d.kind, Kind::Command(_)) && matches!(d.name, "pdfobj" | "pdfxform" | "pdfximage")
}

/// `\immediate` followed by the primitive `id` (D2 calls this): returns false
/// if `id` is not one of ours, else performs it with the object marked immediate.
pub fn immediate(c: &mut dyn Ctx, id: PrimId) -> bool {
    if !immediate_able(id) {
        return false;
    }
    match registry().get(id).code {
        OBJ => do_obj(c, true),
        XFORM => do_xform(c, true),
        XIMAGE => do_ximage(c, true),
        _ => return false,
    }
    true
}

fn command(c: &mut dyn Ctx, code: u16) {
    match code {
        LITERAL => do_literal(c),
        OBJ => do_obj(c, false),
        REFOBJ => do_refobj(c),
        XFORM => do_xform(c, false),
        REFXFORM => do_refxform(c),
        XIMAGE => do_ximage(c, false),
        REFXIMAGE => do_refximage(c),
        ANNOT => do_annot(c),
        STARTLINK => do_startlink(c),
        ENDLINK => {
            if is_vertical(c) {
                error(c, "pdfTeX error (ext1): \\pdfendlink cannot be used in vertical mode", &[]);
                return;
            }
            what(c, PdfWhat::EndLink);
        }
        DEST => do_dest(c),
        OUTLINE => do_outline(c),
        THREAD => do_thread(c, false),
        STARTTHREAD => do_thread(c, true),
        ENDTHREAD => what(c, PdfWhat::EndThread),
        INFO => append_text(c, K_INFO),
        CATALOG => {
            append_text(c, K_CATALOG);
            if scan_keyword(c, "openaction") {
                let k = Key::Other(tag::PDF, K_OPENACTION);
                let had = !matches!(c.get(&k), Val::Void);
                let a = scan_action(c);
                if had {
                    error(c, "pdfTeX error (ext1): duplicate of openaction", &[]);
                } else if let Some(a) = a {
                    c.define(k, Val::Data(Arc::new(a)), true);
                }
            }
        }
        NAMES => append_text(c, K_NAMES),
        TRAILER => append_text(c, K_TRAILER),
        SAVEPOS => what(c, PdfWhat::SavePos),
        COLORSTACK => do_colorstack(c),
        SETMATRIX => {
            let s = ext_str(c);
            what(c, PdfWhat::SetMatrix(s));
        }
        SAVE => what(c, PdfWhat::Save),
        RESTORE => what(c, PdfWhat::Restore),
        SETRANDOMSEED => {
            let n = scan_int(c);
            store_rng(c, &Rng::new(n));
        }
        RESETTIMER => {
            let now = c.int(&Key::Given(2));
            c.define(Key::PdfLast(L_TIMER), Val::Int(now), true);
        }
        G2U => {
            let glyph = ext_str(c);
            let unicode = ext_str(c);
            let n = bump(c, L_G2U);
            let g = GlyphToUnicode { glyph, unicode };
            c.define(Key::Other(tag::PDF, K_G2U | n), Val::Data(Arc::new(g)), true);
        }
        MAPFILE | MAPLINE => {
            let text = ext_str(c);
            let n = bump(c, L_MAPS);
            let e = MapEntry { file: code == MAPFILE, text };
            c.define(Key::Other(tag::PDF, K_MAP | n), Val::Data(Arc::new(e)), true);
        }
        NOLIG => {
            let f = scan_font_ident(c);
            c.define(Key::Other(tag::PDF, K_NOLIG | f), Val::Int(1), true);
        }
        QUITVMODE => {
            if is_vertical(c) {
                crate::list::new_graf(c, true);
            }
        }
        _ => {}
    }
}

fn expand(c: &mut dyn Ctx, code: u16) {
    match code {
        X_UNIFORM => {
            let n = scan_int(c);
            let mut r = load_rng(c);
            let v = r.unif_rand(n);
            store_rng(c, &r);
            push_str(c, &v.to_string());
        }
        X_NORMAL => {
            let mut r = load_rng(c);
            let v = r.norm_rand();
            store_rng(c, &r);
            push_str(c, &v.to_string());
        }
        X_CSINIT => {
            let page = scan_keyword(c, "page");
            let direct = scan_keyword(c, "direct");
            let literal = ext_str(c);
            let n = bump(c, L_COLORSTACKS);
            let init = StackInit { page, direct, literal };
            c.define(Key::Other(tag::PDF, K_CSINIT | n), Val::Data(Arc::new(init)), true);
            push_str(c, &n.to_string());
        }
        X_PAGEREF => {
            let n = scan_int(c);
            if n <= 0 {
                error(c, "pdfTeX error (ext1): invalid page number", &[]);
                return;
            }
            let v = c.int(&Key::Other(tag::PDF, K_PAGEREF | (n as u32 & 0x0fff_ffff)));
            push_str(c, &v.to_string());
        }
        X_PRIMITIVE => do_primitive(c),
        _ => {}
    }
}

fn last_item(c: &mut dyn Ctx, code: u16) -> Val {
    match code {
        C_RANDOMSEED => Val::Int(load_rng(c).seed),
        C_ELAPSED => {
            let now = c.int(&Key::Given(2));
            let base = c.int(&Key::PdfLast(L_TIMER));
            Val::Int(now.saturating_sub(base).max(0))
        }
        n => Val::Int(c.int(&Key::PdfLast(n as u8))),
    }
}

// ---- helpers

fn is_vertical(c: &mut dyn Ctx) -> bool {
    matches!(c.mode(), Mode::Vertical { .. })
}

fn what(c: &mut dyn Ctx, w: PdfWhat) {
    c.item(Item::Whatsit(Arc::new(Whatsit::Pdf(Arc::new(w)))));
}

/// Increments the global counter in slot `s`, returning the new value.
fn bump(c: &mut dyn Ctx, s: u8) -> u32 {
    let n = c.int(&Key::PdfLast(s)) + 1;
    c.define(Key::PdfLast(s), Val::Int(n), true);
    n as u32
}

/// A fresh abstract object id.
fn alloc(c: &mut dyn Ctx) -> u32 {
    bump(c, L_IDS)
}

fn obj_key(id: u32) -> Key {
    Key::Other(tag::PDF, id)
}

fn get_obj(c: &mut dyn Ctx, id: i32) -> Option<PdfObj> {
    if id <= 0 || id as u32 >= K_NOLIG {
        return None;
    }
    data::<PdfObj>(&c.get(&obj_key(id as u32)))
}

fn set_obj(c: &mut dyn Ctx, id: u32, o: PdfObj) {
    c.define(obj_key(id), Val::Data(Arc::new(o)), true);
}

/// `scan_pdf_ext_toks`: a general text, expanded.
fn ext_toks(c: &mut dyn Ctx) -> Toks {
    scan_toks(c, false, true).1
}

fn ext_str(c: &mut dyn Ctx) -> Arc<str> {
    let t = ext_toks(c);
    toks_to_str(c, &t).into()
}

fn str_toks(s: &str) -> Toks {
    s.chars().map(|ch| Tok::Char { cat: if ch == ' ' { 10 } else { 12 }, ch }).collect::<Vec<_>>().into()
}

fn push_str(c: &mut dyn Ctx, s: &str) {
    c.push(str_toks(s), TokKind::Inserted);
}

/// `tokens_to_string` (as `show_token_list` prints, no limit).
pub fn toks_to_str(c: &mut dyn Ctx, t: &[Tok]) -> String {
    let esc = c.int(&Key::IntPar(ip::of("escapechar")));
    let esc = if (0..256).contains(&esc) { char::from_u32(esc as u32) } else { None };
    let mut s = String::new();
    for tok in t {
        match *tok {
            Tok::Char { cat, ch } => {
                s.push(ch);
                if cat == 6 {
                    s.push(ch);
                }
            }
            Tok::Active(ch) => s.push(ch),
            Tok::Cs(cs) => {
                let name = c.names().name(cs);
                s.extend(esc);
                let mut it = name.chars();
                match (it.next(), it.next()) {
                    (None, _) => {
                        s.push_str("csname");
                        s.extend(esc);
                        s.push_str("endcsname ");
                    }
                    (Some(ch), None) => {
                        s.push(ch);
                        if c.int(&Key::Cat(ch)) == 11 {
                            s.push(' ');
                        }
                    }
                    _ => {
                        s.push_str(&name);
                        s.push(' ');
                    }
                }
            }
            Tok::Param(n) => {
                s.push('#');
                if n > 0 {
                    s.push((b'0' + n) as char);
                }
            }
            Tok::DontExpand => {}
        }
    }
    s
}

/// `width`/`height`/`depth` in any order, repeated (`scan_alt_rule`).
fn scan_rule_spec(c: &mut dyn Ctx, w: i32, h: i32, d: i32) -> (i32, i32, i32) {
    let (mut w, mut h, mut d) = (w, h, d);
    loop {
        if scan_keyword(c, "width") {
            w = scan_normal_dimen(c);
        } else if scan_keyword(c, "height") {
            h = scan_normal_dimen(c);
        } else if scan_keyword(c, "depth") {
            d = scan_normal_dimen(c);
        } else {
            return (w, h, d);
        }
    }
}

fn opt_attr(c: &mut dyn Ctx) -> Option<Arc<str>> {
    if scan_keyword(c, "attr") { Some(ext_str(c)) } else { None }
}

/// `\pdfinfo` & co: the job text grows by the expanded text.
fn append_text(c: &mut dyn Ctx, k: u32) {
    let s = ext_str(c);
    let key = Key::Other(tag::PDF, k);
    let old = match c.get(&key) {
        Val::Name(n) => n,
        _ => "".into(),
    };
    let mut t = String::with_capacity(old.len() + s.len());
    t.push_str(&old);
    t.push_str(&s);
    c.define(key, Val::Name(t.into()), true);
}

// ---- \pdfliteral

fn do_literal(c: &mut dyn Ctx) {
    let ship = scan_keyword(c, "shipout");
    let mode = if scan_keyword(c, "direct") {
        LiteralMode::Direct
    } else if scan_keyword(c, "page") {
        LiteralMode::Page
    } else {
        LiteralMode::SetOrigin
    };
    if ship {
        let t = scan_toks(c, false, false).1;
        what(c, PdfWhat::Literal { mode, text: "".into(), ship: Some(t) });
    } else {
        let text = ext_str(c);
        what(c, PdfWhat::Literal { mode, text, ship: None });
    }
}

// ---- objects

/// `reserveobjnum` / `useobjnum n` / a fresh id. None: reserved only (done).
fn obj_id(c: &mut dyn Ctx, last: u8) -> Option<u32> {
    if scan_keyword(c, "reserveobjnum") {
        let id = alloc(c);
        set_obj(c, id, PdfObj::Reserved);
        c.define(Key::PdfLast(last), Val::Int(id as i32), true);
        return None;
    }
    if scan_keyword(c, "useobjnum") {
        let n = scan_int(c);
        if let Some(PdfObj::Reserved) = get_obj(c, n) {
            return Some(n as u32);
        }
        error(c, "pdfTeX error (ext1): invalid object number being reused", &[]);
    }
    Some(alloc(c))
}

fn do_obj(c: &mut dyn Ctx, immediate: bool) {
    let Some(id) = obj_id(c, L_OBJ) else { return };
    let stream = scan_keyword(c, "stream");
    let attr = if stream { opt_attr(c) } else { None };
    let file = scan_keyword(c, "file");
    let text = ext_str(c);
    let data = if file {
        match c.file(&text) {
            Some((name, bytes)) => ObjData::File { name, bytes },
            None => {
                error(c, &format!("pdfTeX error (ext1): cannot open file for embedding: {text}"), &[]);
                ObjData::Text("".into())
            }
        }
    } else {
        ObjData::Text(text)
    };
    set_obj(c, id, PdfObj::Obj { immediate, stream, attr, data });
    c.define(Key::PdfLast(L_OBJ), Val::Int(id as i32), true);
}

fn do_refobj(c: &mut dyn Ctx) {
    let n = scan_int(c);
    match get_obj(c, n) {
        Some(o @ PdfObj::Obj { .. }) => what(c, PdfWhat::RefObj { id: n as u32, obj: Arc::new(o) }),
        _ => error(c, "pdfTeX error (ext1): invalid object number", &[]),
    }
}

fn do_xform(c: &mut dyn Ctx, immediate: bool) {
    let attr = opt_attr(c);
    let resources = if scan_keyword(c, "resources") { Some(ext_str(c)) } else { None };
    let n = scan_register_num(c);
    let b = match c.get(&Key::Box(n)) {
        Val::Box(Some(b)) => b,
        _ => {
            error(c, "pdfTeX error (ext1): \\pdfxform cannot be used with a void box", &[]);
            return;
        }
    };
    // `box(n) := null`: the register is emptied at the current level.
    c.define(Key::Box(n), Val::Box(None), false);
    let id = alloc(c);
    set_obj(c, id, PdfObj::Xform { immediate, attr, resources, b });
    c.define(Key::PdfLast(L_XFORM), Val::Int(id as i32), true);
}

fn do_refxform(c: &mut dyn Ctx) {
    let n = scan_int(c);
    match get_obj(c, n) {
        Some(PdfObj::Xform { immediate, attr, resources, b }) => {
            let (width, height, depth) = (b.width, b.height, b.depth);
            let form = Arc::new(PdfObj::Xform { immediate, attr, resources, b });
            what(c, PdfWhat::RefXform { id: n as u32, width, height, depth, form });
        }
        _ => error(c, "pdfTeX error (ext1): invalid object number", &[]),
    }
}

/// `ext_xn_over_d`: round(x·n/d), half away from zero.
fn xn_over_d(x: i32, n: i32, d: i32) -> i32 {
    if d == 0 {
        return 0;
    }
    let p = x as i128 * n as i128;
    let dd = (d as i128).abs();
    let q = (p.abs() * 2 + dd) / (2 * dd);
    (if (p < 0) != (d < 0) { -q } else { q }) as i32
}

fn do_ximage(c: &mut dyn Ctx, immediate: bool) {
    let (mut w, mut h, mut d) = scan_rule_spec(c, RUNNING, RUNNING, RUNNING);
    let attr = opt_attr(c);
    let mut named = None;
    let mut page = 1;
    if scan_keyword(c, "named") {
        named = Some(ext_str(c));
    } else if scan_keyword(c, "page") {
        page = scan_int(c);
    }
    let colorspace = if scan_keyword(c, "colorspace") { scan_int(c) } else { 0 };
    let pagebox = if scan_keyword(c, "mediabox") {
        1
    } else if scan_keyword(c, "cropbox") {
        2
    } else if scan_keyword(c, "bleedbox") {
        3
    } else if scan_keyword(c, "trimbox") {
        4
    } else if scan_keyword(c, "artbox") {
        5
    } else {
        let p = c.int(&Key::IntPar(ip::of("pdfpagebox")));
        if (1..=5).contains(&p) { p as u8 } else { 2 }
    };
    let fname = ext_str(c);
    let Some((name, bytes)) = c.file(&fname) else {
        error(c, &format!("pdfTeX error (\\pdfximage): cannot find image file '{fname}'"), &[]);
        return;
    };
    let res = c.int(&Key::IntPar(ip::of("pdfimageresolution")));
    let mut iparams = crate::image::Params::read(c);
    iparams.image_resolution = res;
    // GAP: `named` pages are not looked up by image.rs (page number only).
    let req = crate::image::Request { page: page as i32, page_box: i32::from(pagebox) };
    let info = crate::image::info_with(&bytes, &req, &iparams);
    if let Some(e) = &info.error {
        error(c, &format!("pdfTeX error (\\pdfximage): {e}"), &[]);
        return;
    }
    let (x, y) = (info.width, info.height);
    // pdftex `scale_image`
    if w == RUNNING && h == RUNNING && d == RUNNING {
        w = x;
        h = y;
        d = 0;
    } else if w == RUNNING {
        if h == RUNNING {
            w = xn_over_d(y, x, y);
            h = y - d;
        } else if d == RUNNING {
            w = xn_over_d(h, x, y);
            d = 0;
        } else {
            w = xn_over_d(h + d, x, y);
        }
    } else if h == RUNNING && d == RUNNING {
        h = xn_over_d(w, y, x);
        d = 0;
    } else if h == RUNNING {
        h = xn_over_d(w, y, x) - d;
    } else if d == RUNNING {
        d = 0;
    }
    let id = alloc(c);
    let img = Image {
        immediate,
        name,
        bytes,
        page,
        named,
        pagebox,
        colorspace,
        attr,
        width: w,
        height: h,
        depth: d,
        pages: info.pages,
        colordepth: i32::from(info.bits),
    };
    set_obj(c, id, PdfObj::Image(Arc::new(img)));
    c.define(Key::PdfLast(L_XIMAGE), Val::Int(id as i32), true);
    c.define(Key::PdfLast(L_XIMAGE_PAGES), Val::Int(info.pages), true);
    c.define(Key::PdfLast(L_XIMAGE_DEPTH), Val::Int(i32::from(info.bits)), true);
}

fn do_refximage(c: &mut dyn Ctx) {
    let n = scan_int(c);
    match get_obj(c, n) {
        Some(PdfObj::Image(i)) => {
            let (width, height, depth) = (i.width, i.height, i.depth);
            what(c, PdfWhat::RefXimage { id: n as u32, width, height, depth, image: Arc::new(PdfObj::Image(i)) });
        }
        _ => error(c, "pdfTeX error (ext1): invalid image number", &[]),
    }
}

// ---- annotations, links, destinations, threads, outlines

fn do_annot(c: &mut dyn Ctx) {
    let Some(id) = obj_id(c, L_ANNOT) else { return };
    let (width, height, depth) = scan_rule_spec(c, RUNNING, RUNNING, RUNNING);
    let data = ext_str(c);
    c.define(Key::PdfLast(L_ANNOT), Val::Int(id as i32), true);
    what(c, PdfWhat::Annot { id, width, height, depth, data });
}

fn scan_id(c: &mut dyn Ctx) -> Option<DestId> {
    if scan_keyword(c, "num") {
        let n = scan_int(c);
        if n <= 0 {
            error(c, "pdfTeX error (ext1): num identifier must be positive", &[]);
            return None;
        }
        Some(DestId::Num(n))
    } else if scan_keyword(c, "name") {
        Some(DestId::Name(ext_str(c)))
    } else {
        error(c, "pdfTeX error (ext1): identifier type missing", &[]);
        None
    }
}

/// pdftex `scan_action`.
pub fn scan_action(c: &mut dyn Ctx) -> Option<Action> {
    let goto = if scan_keyword(c, "user") {
        return Some(Action::User(ext_str(c)));
    } else if scan_keyword(c, "goto") {
        true
    } else if scan_keyword(c, "thread") {
        false
    } else {
        error(c, "pdfTeX error (ext1): action type missing", &[]);
        return None;
    };
    let file = if scan_keyword(c, "file") { Some(ext_str(c)) } else { None };
    let target = if goto && scan_keyword(c, "page") {
        let page = scan_int(c);
        if page <= 0 {
            error(c, "pdfTeX error (ext1): page number must be positive", &[]);
        }
        let spec = ext_str(c);
        Err(GotoTarget::Page { page: page.max(1), spec })
    } else {
        Ok(scan_id(c)?)
    };
    let new_window = if scan_keyword(c, "newwindow") {
        Some(true)
    } else if scan_keyword(c, "nonewwindow") {
        Some(false)
    } else {
        None
    };
    Some(match (goto, target) {
        (_, Err(target)) => Action::Goto { file, target, new_window },
        (true, Ok(DestId::Num(n))) => Action::Goto { file, target: GotoTarget::Num(n), new_window },
        (true, Ok(DestId::Name(s))) => Action::Goto { file, target: GotoTarget::Name(s), new_window },
        (false, Ok(id)) => Action::Thread { file, id, new_window },
    })
}

fn do_startlink(c: &mut dyn Ctx) {
    if is_vertical(c) {
        error(c, "pdfTeX error (ext1): \\pdfstartlink cannot be used in vertical mode", &[]);
        return;
    }
    let (width, height, depth) = scan_rule_spec(c, RUNNING, RUNNING, RUNNING);
    let attr = opt_attr(c);
    let Some(action) = scan_action(c) else { return };
    let id = alloc(c);
    c.define(Key::PdfLast(L_LINK), Val::Int(id as i32), true);
    what(c, PdfWhat::StartLink { id, width, height, depth, attr, action: Arc::new(action) });
}

fn do_dest(c: &mut dyn Ctx) {
    let Some(id) = scan_id(c) else { return };
    let (mut w, mut h, mut d) = (RUNNING, RUNNING, RUNNING);
    let kind = if scan_keyword(c, "xyz") {
        let zoom = if scan_keyword(c, "zoom") { Some(scan_int(c)) } else { None };
        DestKind::Xyz { zoom }
    } else if scan_keyword(c, "fitbh") {
        DestKind::FitBH
    } else if scan_keyword(c, "fitbv") {
        DestKind::FitBV
    } else if scan_keyword(c, "fitb") {
        DestKind::FitB
    } else if scan_keyword(c, "fith") {
        DestKind::FitH
    } else if scan_keyword(c, "fitv") {
        DestKind::FitV
    } else if scan_keyword(c, "fitr") {
        (w, h, d) = scan_rule_spec(c, w, h, d);
        DestKind::FitR
    } else if scan_keyword(c, "fit") {
        DestKind::Fit
    } else {
        error(c, "pdfTeX error (ext1): destination type missing", &[]);
        return;
    };
    what(c, PdfWhat::Dest { id, kind, width: w, height: h, depth: d });
}

fn do_thread(c: &mut dyn Ctx, start: bool) {
    let (width, height, depth) = scan_rule_spec(c, RUNNING, RUNNING, RUNNING);
    let attr = opt_attr(c);
    let Some(id) = scan_id(c) else { return };
    what(c, PdfWhat::Thread { start, id, attr, width, height, depth });
}

fn do_outline(c: &mut dyn Ctx) {
    let attr = opt_attr(c);
    let Some(action) = scan_action(c) else { return };
    let count = if scan_keyword(c, "count") { scan_int(c) } else { 0 };
    let title = ext_str(c);
    let n = bump(c, L_OUTLINES);
    let o = Outline { attr, action, count, title };
    c.define(Key::Other(tag::PDF, K_OUTLINE | n), Val::Data(Arc::new(o)), true);
}

// ---- colour stacks

fn default_stack() -> StackInit {
    StackInit { page: true, direct: false, literal: "0 g 0 G".into() }
}

fn stack_init(c: &mut dyn Ctx, n: u32) -> Option<StackInit> {
    if n == 0 {
        return Some(default_stack());
    }
    data::<StackInit>(&c.get(&Key::Other(tag::PDF, K_CSINIT | n)))
}

fn do_colorstack(c: &mut dyn Ctx) {
    let n = scan_int(c);
    let init = if n >= 0 && (n as u32) < 0x0fff_ffff { stack_init(c, n as u32) } else { None };
    let Some(init) = init else {
        error(c, "pdfTeX error (ext1): Unknown color stack number", &[]);
        return;
    };
    let op = if scan_keyword(c, "set") {
        StackOp::Set
    } else if scan_keyword(c, "push") {
        StackOp::Push
    } else if scan_keyword(c, "pop") {
        StackOp::Pop
    } else if scan_keyword(c, "current") {
        StackOp::Current
    } else {
        error(c, "pdfTeX error (ext1): Color stack action is missing", &[]);
        return;
    };
    let text = if matches!(op, StackOp::Set | StackOp::Push) { ext_str(c) } else { "".into() };
    what(c, PdfWhat::ColorStack { stack: n as u32, op, text, init: Arc::new(init) });
}

// ---- \pdfprimitive

fn do_primitive(c: &mut dyn Ctx) {
    let Some(t) = c.next() else { return };
    let Tok::Cs(cs) = t else {
        error(c, "pdfTeX error (\\pdfprimitive): missing primitive name", &[]);
        c.back(t);
        return;
    };
    let name = c.names().name(cs);
    let reg = registry();
    let Some(id) = reg.by_name(&name) else {
        error(c, "pdfTeX error (\\pdfprimitive): missing primitive name", &[]);
        return;
    };
    let eng = reg.get(id).engine;
    if (eng == 1 && !c.etex()) || (eng == 2 && !c.pdf()) {
        error(c, "pdfTeX error (\\pdfprimitive): missing primitive name", &[]);
        return;
    }
    if c.meaning(&t) == Meaning::Prim(id) {
        c.back(t);
        return;
    }
    // A name frozen to the primitive (cannot be typed: it starts with NUL).
    let hidden = c.names().intern(&format!("\u{0}{name}"));
    let k = Key::Cs(hidden);
    if c.get(&k) != Val::Meaning(Meaning::Prim(id)) {
        c.define(k, Val::Meaning(Meaning::Prim(id)), true);
    }
    c.back(Tok::Cs(hidden));
}

// ---- random numbers (MetaPost's generator, as pdfTeX uses it)

const FRACTION_ONE: i64 = 1 << 28;
const FRACTION_HALF: i64 = 1 << 27;
const FRACTION_FOUR: i64 = 1 << 30;

#[derive(Clone)]
struct Rng {
    seed: i32,
    j: usize,
    a: [i64; 55],
}

fn take_fraction(q: i64, f: i64) -> i64 {
    let p = q * f;
    let r = (p.abs() + FRACTION_HALF) >> 28;
    if p < 0 { -r } else { r }
}

fn make_fraction(p: i64, q: i64) -> i64 {
    let n = p << 28;
    let r = (n.abs() + q.abs() / 2) / q.abs();
    if (n < 0) != (q < 0) { -r } else { r }
}

fn half(x: i64) -> i64 {
    if x & 1 != 0 { (x + 1).div_euclid(2) } else { x.div_euclid(2) }
}

const SPEC_LOG: [i64; 29] = [
    0, 93032640, 38612034, 17922280, 8662214, 4261238, 2113709, 1052693, 525315, 262400, 131136, 65552,
    32772, 16385, 8192, 4096, 2048, 1024, 512, 256, 128, 64, 32, 16, 8, 4, 2, 1, 1,
];

/// MetaPost `m_log`: 2^24·ln(x/2^16).
fn m_log(x: i64) -> i64 {
    if x <= 0 {
        return 0;
    }
    let mut x = x;
    let mut y: i64 = 1302456956 + 4 - 100;
    let mut z: i64 = 27595 + 6553600;
    while x < FRACTION_FOUR {
        x *= 2;
        y -= 93032639;
        z -= 48782;
    }
    y += z / 65536;
    let mut k = 2usize;
    while x > FRACTION_FOUR + 4 {
        let mut zz = (x - 1) / (1i64 << k) + 1;
        while x < FRACTION_FOUR + zz {
            zz = half(zz + 1);
            k += 1;
        }
        y += SPEC_LOG[k.min(28)];
        x -= zz;
    }
    y / 8
}

impl Rng {
    fn new(seed: i32) -> Rng {
        let mut r = Rng { seed, j: 0, a: [0; 55] };
        let mut j = (seed as i64).abs();
        while j >= FRACTION_ONE {
            j = half(j);
        }
        let mut k: i64 = 1;
        for i in 0..55 {
            let jj = k;
            k = j - k;
            j = jj;
            if k < 0 {
                k += FRACTION_ONE;
            }
            r.a[(i * 21) % 55] = j;
        }
        r.new_randoms();
        r.new_randoms();
        r.new_randoms();
        r
    }
    fn new_randoms(&mut self) {
        for k in 0..24 {
            let mut x = self.a[k] - self.a[k + 31];
            if x < 0 {
                x += FRACTION_ONE;
            }
            self.a[k] = x;
        }
        for k in 24..55 {
            let mut x = self.a[k] - self.a[k - 24];
            if x < 0 {
                x += FRACTION_ONE;
            }
            self.a[k] = x;
        }
        self.j = 54;
    }
    fn next(&mut self) -> i64 {
        if self.j == 0 {
            self.new_randoms();
        } else {
            self.j -= 1;
        }
        self.a[self.j]
    }
    fn unif_rand(&mut self, x: i32) -> i32 {
        let ax = (x as i64).abs();
        let r = self.next();
        let y = take_fraction(ax, r);
        if y == ax {
            0
        } else if x > 0 {
            y as i32
        } else {
            -y as i32
        }
    }
    fn norm_rand(&mut self) -> i32 {
        loop {
            let (x, u) = loop {
                let r = self.next();
                let x = take_fraction(112429, r - FRACTION_HALF);
                let u = self.next();
                if x.abs() < u {
                    break (x, u);
                }
            };
            let x = make_fraction(x, u);
            let l = 139548960 - m_log(u);
            if 1024 * l >= x * x {
                return x as i32;
            }
        }
    }
}

/// The generator's state: `Key::PdfRandom` = [seed, j, a[0..55]]; void before
/// first use, then seeded from the build's given seed (`Key::Given(1)`).
fn load_rng(c: &mut dyn Ctx) -> Rng {
    if let Val::Ints(v) = c.get(&Key::PdfRandom) {
        if v.len() == 57 {
            let mut a = [0i64; 55];
            for (i, x) in v[2..].iter().enumerate() {
                a[i] = *x as i64;
            }
            return Rng { seed: v[0], j: v[1] as usize, a };
        }
    }
    let seed = c.int(&Key::Given(1));
    Rng::new(seed)
}

fn store_rng(c: &mut dyn Ctx, r: &Rng) {
    let mut v = Vec::with_capacity(57);
    v.push(r.seed);
    v.push(r.j as i32);
    v.extend(r.a.iter().map(|&x| x as i32));
    c.define(Key::PdfRandom, Val::Ints(v.into()), true);
}

// ---- the job-level material, for E1

/// Everything document-wide the pdfTeX primitives made.
#[derive(Clone, Debug, Default)]
pub struct Job {
    /// Every object by id (1..): reserved, `\pdfobj`, `\pdfxform`, `\pdfximage`.
    /// E1 writes the immediate ones and those a shipped whatsit references.
    pub objs: Vec<(u32, Arc<PdfObj>)>,
    pub outlines: Vec<Outline>,
    pub info: Arc<str>,
    pub catalog: Arc<str>,
    pub names: Arc<str>,
    pub trailer: Arc<str>,
    pub open_action: Option<Action>,
    pub glyph_to_unicode: Vec<GlyphToUnicode>,
    pub maps: Vec<MapEntry>,
    /// Colour stacks by number (0 the predefined one).
    pub colorstacks: Vec<StackInit>,
    /// Abstract ids allocated (objects, annots, links), 1..=ids.
    pub ids: u32,
}

/// Gathers the job-level material from the env at the job's end.
pub fn job(e: &Env) -> Job {
    let name = |k: u32| match e.lookup(&Key::Other(tag::PDF, k)) {
        Val::Name(n) => n,
        _ => Arc::from(""),
    };
    let count = |s: u8| e.lookup(&Key::PdfLast(s)).int().max(0) as u32;
    let mut j = Job {
        info: name(K_INFO),
        catalog: name(K_CATALOG),
        names: name(K_NAMES),
        trailer: name(K_TRAILER),
        open_action: data::<Action>(&e.lookup(&Key::Other(tag::PDF, K_OPENACTION))),
        ids: count(L_IDS),
        ..Job::default()
    };
    for id in 1..=j.ids {
        if let Some(o) = data::<PdfObj>(&e.lookup(&obj_key(id))) {
            j.objs.push((id, Arc::new(o)));
        }
    }
    for i in 1..=count(L_OUTLINES) {
        if let Some(o) = data::<Outline>(&e.lookup(&Key::Other(tag::PDF, K_OUTLINE | i))) {
            j.outlines.push(o);
        }
    }
    for i in 1..=count(L_G2U) {
        if let Some(g) = data::<GlyphToUnicode>(&e.lookup(&Key::Other(tag::PDF, K_G2U | i))) {
            j.glyph_to_unicode.push(g);
        }
    }
    for i in 1..=count(L_MAPS) {
        if let Some(m) = data::<MapEntry>(&e.lookup(&Key::Other(tag::PDF, K_MAP | i))) {
            j.maps.push(m);
        }
    }
    j.colorstacks.push(default_stack());
    for n in 1..=count(L_COLORSTACKS) {
        if let Some(s) = data::<StackInit>(&e.lookup(&Key::Other(tag::PDF, K_CSINIT | n))) {
            j.colorstacks.push(s);
        }
    }
    j
}

/// `\pdfnoligatures` was given for font `f` (a recorded read; for C1/C7).
pub fn no_ligatures(c: &mut dyn Ctx, f: crate::val::FontId) -> bool {
    c.int(&Key::Other(tag::PDF, K_NOLIG | f)) != 0
}
