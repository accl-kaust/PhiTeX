//! E1: the PDF file from shipped pages, as pdfTeX's `pdf_ship_out` writes it:
//! page content streams (`hlist_out`/`vlist_out` with pdfTeX's text
//! positioning, `\pdfdecimaldigits` numbers, rules, glue rounding, leaders),
//! page dictionaries and resources, pdfTeX whatsits (literals, colour stacks,
//! matrices, xforms, images, annotations, links across lines, dests, threads),
//! outlines, names, info, catalog, object streams and the xref with `/ID`.
//!
//! Object numbers: D3's abstract ids (`\pdflastobj` …, 1..=ids) are the
//! object numbers themselves, so user text such as `\the\pdflastobj\space 0 R`
//! stays valid; everything else is allocated above them.
//!
//! Incremental: a page's content stream is a pure function of its shipped
//! box (whatsits carry their xforms/images), the parameters read at shipout
//! and the state carried in (colour stacks, an open thread). [`Pdf`] keeps
//! every page's result and reuses it when those are equal; the file is then
//! re-assembled from the cached, already compressed streams.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::sync::Arc;

use crate::item::{BoxNode, Item, Whatsit, RUNNING};
use crate::output::Shipped;
use crate::pdfprim::{Action, DestId, DestKind, GotoTarget, LiteralMode, ObjData, PdfObj, PdfWhat, StackInit, StackOp};
use crate::prim::{dp, ip, tp};
use crate::tok::{Names, Tok, Toks};
use crate::val::{FontId, Key, Val};

// ------------------------------------------------------------------ the job

/// Everything the file is made of.
pub struct Job<'a> {
    pub pages: &'a [Arc<Shipped>],
    /// Parameters at the end of the job (compression, version, info flags).
    pub params: &'a [(Key, Val)],
    pub names: &'a Names,
    /// D3's job-level material (`pdfprim::job(&env)`).
    pub pdf: &'a crate::pdfprim::Job,
    /// `D:YYYYMMDDHHmmSS+HH'mm'` of the build (empty: none).
    pub creation_date: Arc<str>,
    /// Bytes hashed into `/ID` (job name, date, …).
    pub id_seed: Vec<u8>,
    /// Support files for fonts (map, enc, pfb, vf …) by name.
    pub loader: crate::pdffont::Loader,
}

/// The file and what the job reads back from it.
#[derive(Clone, Debug, Default)]
pub struct Built {
    pub bytes: Vec<u8>,
    pub warnings: Vec<String>,
    /// `\pdfsavepos` positions per page, in sp from the page's lower left
    /// (`\pdflastxpos`/`\pdflastypos`).
    pub saved: Vec<Vec<(i32, i32)>>,
    /// Page object numbers (`\pdfpageref`, `K_PAGEREF`).
    pub page_objs: Vec<u32>,
}

// ------------------------------------------------------------------ the object writer

#[derive(Clone, Copy, Debug)]
enum Loc {
    Free,
    At(usize),
    InStm(u32, u32),
}

/// Writes objects: direct ones at once, non-stream ones into object
/// streams when `\pdfobjcompresslevel` > 0.
pub struct Writer {
    pub out: Vec<u8>,
    next: u32,
    pub compress_level: i32,
    objstm: bool,
    locs: Vec<Loc>,
    pending: Vec<(u32, Vec<u8>)>,
}

impl Writer {
    pub fn new(minor: i32, compress_level: i32, obj_compress: i32, first_free: u32) -> Writer {
        let mut out = format!("%PDF-1.{minor}\n").into_bytes();
        out.extend_from_slice(b"%\xD0\xD4\xC5\xD8\n");
        Writer {
            out,
            next: first_free.max(1),
            compress_level: compress_level.clamp(0, 9),
            objstm: obj_compress > 0 && minor >= 5 && compress_level > 0,
            locs: Vec::new(),
            pending: Vec::new(),
        }
    }

    /// Reserves an object number.
    pub fn alloc(&mut self) -> u32 {
        let n = self.next;
        self.next += 1;
        n
    }

    fn set_loc(&mut self, num: u32, l: Loc) {
        let i = num as usize;
        if self.locs.len() <= i {
            self.locs.resize(i + 1, Loc::Free);
        }
        self.locs[i] = l;
        if num >= self.next {
            self.next = num + 1;
        }
    }

    fn direct(&mut self, num: u32, body: &[u8]) {
        let at = self.out.len();
        self.set_loc(num, Loc::At(at));
        self.out.extend_from_slice(format!("{num} 0 obj\n").as_bytes());
        self.out.extend_from_slice(body);
        self.out.extend_from_slice(b"\nendobj\n");
    }

    /// The body of object `num` (between `num 0 obj` and `endobj`). Streams
    /// are written directly; other objects go to an object stream if enabled.
    pub fn write_obj(&mut self, num: u32, body: &[u8]) {
        let is_stream = body.ends_with(b"endstream") || body.ends_with(b"endstream\n");
        if self.objstm && !is_stream {
            self.set_loc(num, Loc::Free);
            self.pending.push((num, body.to_vec()));
            if self.pending.len() >= 100 {
                self.flush_objstm();
            }
        } else {
            self.direct(num, body);
        }
    }

    /// A stream object; `dict` holds extra entries; compressed at the level.
    pub fn write_stream(&mut self, num: u32, dict: &str, data: &[u8]) {
        let (enc, filtered) = encode(data, self.compress_level);
        self.write_encoded(num, dict, &enc, filtered);
    }

    /// A stream whose data is already encoded (`filtered`: Flate).
    pub fn write_encoded(&mut self, num: u32, dict: &str, data: &[u8], filtered: bool) {
        let mut body = Vec::with_capacity(data.len() + 64 + dict.len());
        body.extend_from_slice(b"<<\n");
        if !dict.is_empty() {
            body.extend_from_slice(dict.as_bytes());
            if !dict.ends_with('\n') {
                body.push(b'\n');
            }
        }
        body.extend_from_slice(format!("/Length {}\n", data.len()).as_bytes());
        if filtered {
            body.extend_from_slice(b"/Filter /FlateDecode\n");
        }
        body.extend_from_slice(b">>\nstream\n");
        body.extend_from_slice(data);
        body.extend_from_slice(b"\nendstream");
        self.direct(num, &body);
    }

    fn flush_objstm(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let objs = std::mem::take(&mut self.pending);
        let stm = self.alloc();
        let mut head = String::new();
        let mut data = Vec::new();
        for (i, (num, body)) in objs.iter().enumerate() {
            let _ = write!(head, "{} {} ", num, data.len());
            data.extend_from_slice(body);
            data.push(b'\n');
            self.set_loc(*num, Loc::InStm(stm, i as u32));
        }
        head.push('\n');
        let first = head.len();
        let mut all = head.into_bytes();
        all.extend_from_slice(&data);
        let dict = format!("/Type /ObjStm\n/N {}\n/First {}", objs.len(), first);
        self.write_stream(stm, &dict, &all);
    }

    /// Ends the file: xref (table or stream) and trailer.
    pub fn finish(mut self, root: u32, info: u32, id: &str, trailer_extra: &str) -> Vec<u8> {
        self.flush_objstm();
        if self.objstm {
            let xref = self.alloc();
            let size = self.next;
            let at = self.out.len();
            self.set_loc(xref, Loc::At(at));
            let mut data = Vec::with_capacity(size as usize * 7);
            for n in 0..size {
                match self.locs.get(n as usize).copied().unwrap_or(Loc::Free) {
                    Loc::Free => {
                        data.push(0);
                        data.extend_from_slice(&0u32.to_be_bytes());
                        data.extend_from_slice(&(if n == 0 { 0xffffu16 } else { 0 }).to_be_bytes());
                    }
                    Loc::At(o) => {
                        data.push(1);
                        data.extend_from_slice(&(o as u32).to_be_bytes());
                        data.extend_from_slice(&0u16.to_be_bytes());
                    }
                    Loc::InStm(s, i) => {
                        data.push(2);
                        data.extend_from_slice(&s.to_be_bytes());
                        data.extend_from_slice(&(i as u16).to_be_bytes());
                    }
                }
            }
            let mut dict = format!(
                "/Type /XRef\n/Index [0 {size}]\n/Size {size}\n/W [1 4 2]\n/Root {root} 0 R\n/Info {info} 0 R\n/ID [<{id}> <{id}>]"
            );
            if !trailer_extra.is_empty() {
                dict.push('\n');
                dict.push_str(trailer_extra);
            }
            let (enc, filtered) = encode(&data, self.compress_level);
            self.write_encoded(xref, &dict, &enc, filtered);
            self.out.extend_from_slice(format!("startxref\n{at}\n%%EOF\n").as_bytes());
        } else {
            let size = self.next;
            let at = self.out.len();
            let mut s = format!("xref\n0 {size}\n");
            for n in 0..size {
                match self.locs.get(n as usize).copied().unwrap_or(Loc::Free) {
                    Loc::At(o) => {
                        let _ = write!(s, "{o:010} 00000 n \n");
                    }
                    _ if n == 0 => s.push_str("0000000000 65535 f \n"),
                    _ => s.push_str("0000000000 00000 f \n"),
                }
            }
            let _ = write!(s, "trailer\n<< /Size {size}\n/Root {root} 0 R\n/Info {info} 0 R\n/ID [<{id}> <{id}>]");
            if !trailer_extra.is_empty() {
                s.push('\n');
                s.push_str(trailer_extra);
            }
            let _ = write!(s, " >>\nstartxref\n{at}\n%%EOF\n");
            self.out.extend_from_slice(s.as_bytes());
        }
        self.out
    }
}

fn encode(data: &[u8], level: i32) -> (Vec<u8>, bool) {
    if level > 0 {
        (phitex_codecs::deflate::compress(data, level), true)
    } else {
        (data.to_vec(), false)
    }
}

/// E6's sink: its first allocation is the image's own id, so the XObject
/// gets the number `\pdflastximage` gave the job.
struct ImageSink<'w> {
    w: &'w mut Writer,
    first: Option<u32>,
}

impl crate::image::ObjSink for ImageSink<'_> {
    fn alloc(&mut self) -> u32 {
        match self.first.take() {
            Some(n) => n,
            None => self.w.alloc(),
        }
    }
    fn write_obj(&mut self, num: u32, body: &[u8]) {
        self.w.write_obj(num, body);
    }
}

// ------------------------------------------------------------------ numbers

const POW10: [i64; 5] = [1, 10, 100, 1000, 10000];

/// sp → bp in units of 10^-digits, with `\mag`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Num {
    digits: u32,
    mag: i64,
}

fn div_round(n: i128, d: i128) -> i64 {
    let q = (2 * n.abs() + d) / (2 * d);
    (if n < 0 { -q } else { q }) as i64
}

impl Num {
    fn units(self, s: i64) -> i64 {
        let num = s as i128 * self.mag as i128 * 7200 * POW10[self.digits as usize] as i128;
        div_round(num, 7227 * 65536 * 1000)
    }
    fn fmt(self, n: i64) -> String {
        fmt_units(n, self.digits)
    }
    fn bp(self, s: i64) -> String {
        self.fmt(self.units(s))
    }
    fn to_f(self, n: i64) -> f64 {
        n as f64 / POW10[self.digits as usize] as f64
    }
}

/// pdfTeX's `pdf_print_real`: fixed digits, trailing zeros dropped.
fn fmt_units(n: i64, d: u32) -> String {
    let p = POW10[d as usize];
    let a = n.unsigned_abs() as i64;
    let mut s = String::new();
    if n < 0 {
        s.push('-');
    }
    let _ = write!(s, "{}", a / p);
    let mut fr = a % p;
    if fr != 0 {
        let mut w = d as usize;
        while fr % 10 == 0 {
            fr /= 10;
            w -= 1;
        }
        let _ = write!(s, ".{fr:0w$}");
    }
    s
}

fn fmt_f(x: f64, d: usize) -> String {
    let mut s = format!("{x:.d$}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if s == "-0" {
        s = "0".into();
    }
    s
}

/// TeX's `round` (half away from zero) after `vet_glue`'s clamp.
fn glue_round(x: f64) -> i32 {
    let x = x.clamp(-1_000_000_000.0, 1_000_000_000.0);
    if x >= 0.0 { (x + 0.5).floor() as i32 } else { (x - 0.5).ceil() as i32 }
}

// ------------------------------------------------------------------ parameters

fn pget<'p>(params: &'p [(Key, Val)], k: &Key) -> Option<&'p Val> {
    params.iter().rev().find(|(kk, _)| kk == k).map(|(_, v)| v)
}

fn ipar(params: &[(Key, Val)], name: &str, dflt: i32) -> i32 {
    pget(params, &Key::IntPar(ip::of(name))).map_or(dflt, Val::int)
}

fn dpar(params: &[(Key, Val)], name: &str, dflt: i32) -> i32 {
    pget(params, &Key::DimenPar(dp::of(name))).map_or(dflt, Val::int)
}

fn tpar(params: &[(Key, Val)], name: &str, names: &Names) -> String {
    match pget(params, &Key::TokPar(tp::of(name))) {
        Some(Val::Name(s)) => s.to_string(),
        Some(Val::Toks(t)) => toks_text(t, names),
        _ => String::new(),
    }
}

/// A token list as TeX prints it (`\cs ` with a space after a letter name).
pub fn toks_text(t: &Toks, names: &Names) -> String {
    let mut s = String::new();
    for tok in t.iter() {
        match *tok {
            Tok::Char { ch, .. } | Tok::Active(ch) => s.push(ch),
            Tok::Cs(c) => {
                let n = names.name(c);
                s.push('\\');
                s.push_str(&n);
                if n.chars().count() != 1 || n.chars().all(char::is_alphabetic) {
                    s.push(' ');
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

const ONE_INCH: i32 = 4_736_286;

/// The page-level parameters a content stream depends on.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PagePars {
    num: Num,
    h_off: i32,
    v_off: i32,
    page_w: i32,
    page_h: i32,
    link_margin: i32,
    dest_margin: i32,
    thread_margin: i32,
}

fn page_pars(params: &[(Key, Val)], b: &BoxNode) -> PagePars {
    let digits = ipar(params, "pdfdecimaldigits", 3).clamp(0, 4) as u32;
    let mag = ipar(params, "mag", 1000);
    let mag = if mag <= 0 { 1000 } else { mag } as i64;
    let h_off = dpar(params, "pdfhorigin", ONE_INCH) + dpar(params, "hoffset", 0);
    let v_off = dpar(params, "pdfvorigin", ONE_INCH) + dpar(params, "voffset", 0);
    let pw = dpar(params, "pdfpagewidth", 0);
    let ph = dpar(params, "pdfpageheight", 0);
    PagePars {
        num: Num { digits, mag },
        h_off,
        v_off,
        page_w: if pw != 0 { pw } else { b.width + 2 * h_off },
        page_h: if ph != 0 { ph } else { b.height + b.depth + 2 * v_off },
        link_margin: dpar(params, "pdflinkmargin", 0),
        dest_margin: dpar(params, "pdfdestmargin", 0),
        thread_margin: dpar(params, "pdfthreadmargin", 0),
    }
}

/// A dest/thread id as a map key.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Dk {
    Num(i32),
    Name(Arc<str>),
}

fn dk(id: &DestId) -> Dk {
    match id {
        DestId::Num(n) => Dk::Num(*n),
        DestId::Name(s) => Dk::Name(s.clone()),
    }
}

fn dest_label(id: &Dk) -> String {
    match id {
        Dk::Name(n) => format!("name{{{n}}}"),
        Dk::Num(n) => format!("num{n}"),
    }
}

// ------------------------------------------------------------------ what a page produced

#[derive(Clone, Debug, PartialEq)]
enum AnnotOut {
    /// `\pdfannot` with the job's id.
    Annot { num: u32, rect: String, data: Arc<str> },
    /// One segment of a link; the first carries the job's id.
    Link { num: Option<u32>, rect: String, attr: Option<Arc<str>>, action: Arc<Action> },
}

#[derive(Clone, Debug, PartialEq)]
struct BeadOut {
    id: Dk,
    rect: String,
    attr: Option<Arc<str>>,
}

/// A thread bead open across boxes/pages (`\pdfstartthread`).
#[derive(Clone, Debug, PartialEq)]
pub struct OpenThread {
    id: Dk,
    attr: Option<Arc<str>>,
    w: i32,
    d: i32,
    level: u32,
    /// The open bead on this page: (left, width, top v).
    seg: Option<(i32, i32, i32)>,
}

/// State carried from page to page.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Carry {
    /// Per stack: its values, top last.
    colors: Vec<Vec<Arc<str>>>,
    /// Per stack: page mode.
    page_mode: Vec<bool>,
    thread: Option<OpenThread>,
}

#[derive(Clone, Debug)]
struct PageOut {
    stream: Vec<u8>,
    filtered: bool,
    fonts: BTreeSet<FontId>,
    glyphs: BTreeSet<(FontId, u16)>,
    xforms: BTreeSet<u32>,
    images: BTreeSet<u32>,
    annots: Vec<AnnotOut>,
    dests: Vec<(Dk, String)>,
    beads: Vec<BeadOut>,
    saved: Vec<(i32, i32)>,
    warnings: Vec<String>,
    carry_out: Carry,
    /// Objects the page references (written even if not immediate).
    objs: BTreeMap<u32, Arc<PdfObj>>,
    width: i32,
    height: i32,
}

// ------------------------------------------------------------------ the renderer

struct OpenLink {
    num: Option<u32>,
    attr: Option<Arc<str>>,
    action: Arc<Action>,
    h: i32,
    d: i32,
    level: u32,
    /// Open segment: (start h, baseline, box height, box depth).
    seg: Option<(i32, i32, i32, i32)>,
}

/// The box an item sits in, for running dimensions.
#[derive(Clone, Copy)]
struct Parent {
    vertical: bool,
    left: i32,
    base: i32,
    width: i32,
    height: i32,
    depth: i32,
}

struct R {
    p: PagePars,
    iparams: crate::image::Params,
    s: Vec<u8>,
    in_text: bool,
    in_tj: bool,
    in_str: bool,
    font: Option<FontId>,
    size_bp: f64,
    line_x: i64,
    line_y: i64,
    tj_x: f64,
    fonts: BTreeSet<FontId>,
    glyphs: BTreeSet<(FontId, u16)>,
    xforms: BTreeSet<u32>,
    images: BTreeSet<u32>,
    annots: Vec<AnnotOut>,
    dests: Vec<(Dk, String)>,
    beads: Vec<BeadOut>,
    saved: Vec<(i32, i32)>,
    warnings: Vec<String>,
    objs: BTreeMap<u32, Arc<PdfObj>>,
    carry: Carry,
    links: Vec<OpenLink>,
    level: u32,
    save_depth: u32,
}

impl R {
    fn new(p: PagePars, iparams: crate::image::Params, carry: Carry) -> Self {
        R {
            p,
            iparams,
            s: Vec::new(),
            in_text: false,
            in_tj: false,
            in_str: false,
            font: None,
            size_bp: 0.0,
            line_x: 0,
            line_y: 0,
            tj_x: 0.0,
            fonts: BTreeSet::new(),
            glyphs: BTreeSet::new(),
            xforms: BTreeSet::new(),
            images: BTreeSet::new(),
            annots: Vec::new(),
            dests: Vec::new(),
            beads: Vec::new(),
            saved: Vec::new(),
            warnings: Vec::new(),
            objs: BTreeMap::new(),
            carry,
            links: Vec::new(),
            level: 0,
            save_depth: 0,
        }
    }

    fn xu(&self, h: i32) -> i64 {
        self.p.num.units(h as i64 + self.p.h_off as i64)
    }
    fn yu(&self, v: i32) -> i64 {
        self.p.num.units(self.p.page_h as i64 - v as i64 - self.p.v_off as i64)
    }
    fn f(&self, n: i64) -> String {
        self.p.num.fmt(n)
    }
    fn put(&mut self, t: &str) {
        self.s.extend_from_slice(t.as_bytes());
    }

    fn end_tj(&mut self) {
        if self.in_tj {
            if self.in_str {
                self.s.push(b')');
                self.in_str = false;
            }
            self.put("]TJ\n");
            self.in_tj = false;
        }
    }
    fn end_text(&mut self) {
        self.end_tj();
        if self.in_text {
            self.put("ET\n");
            self.in_text = false;
            self.font = None;
        }
    }

    /// A character at (h, v); returns its width.
    fn char_out(&mut self, font: FontId, c: u16, h: i32, v: i32) -> i32 {
        let f = crate::font::fonts().font(font);
        let width = f.width(c);
        let size = f.size();
        if !self.in_text {
            self.put("BT\n");
            self.in_text = true;
            self.line_x = 0;
            self.line_y = 0;
            self.font = None;
        }
        if self.font != Some(font) {
            self.end_tj();
            let su = self.p.num.units(size as i64);
            let t = format!("/F{} {} Tf\n", font, self.f(su));
            self.put(&t);
            self.font = Some(font);
            self.size_bp = self.p.num.to_f(su);
            self.fonts.insert(font);
        }
        self.glyphs.insert((font, c));
        let tx = self.xu(h);
        let ty = self.yu(v);
        let want = self.p.num.to_f(tx);
        let mut placed = false;
        if self.in_tj && ty == self.line_y && self.size_bp > 0.0 {
            let k = ((self.tj_x - want) * 1000.0 / self.size_bp).round() as i64;
            if k.abs() < 1_000_000 {
                if k != 0 {
                    if self.in_str {
                        self.s.push(b')');
                        self.in_str = false;
                    }
                    let t = format!("{k}");
                    self.put(&t);
                    self.tj_x -= k as f64 * self.size_bp / 1000.0;
                }
                placed = true;
            }
        }
        if !placed {
            self.end_tj();
            let t = format!("{} {} Td\n[", self.f(tx - self.line_x), self.f(ty - self.line_y));
            self.put(&t);
            self.line_x = tx;
            self.line_y = ty;
            self.tj_x = want;
            self.in_tj = true;
            self.in_str = false;
        }
        if !self.in_str {
            self.s.push(b'(');
            self.in_str = true;
        }
        let bytes: Vec<u8> = if c < 256 { vec![c as u8] } else { vec![(c >> 8) as u8, c as u8] };
        for b in bytes {
            match b {
                b'(' | b')' | b'\\' => {
                    self.s.push(b'\\');
                    self.s.push(b);
                }
                0..=31 | 127..=255 => {
                    let t = format!("\\{b:03o}");
                    self.put(&t);
                }
                _ => self.s.push(b),
            }
        }
        if size > 0 {
            let wk = (width as f64 * 1000.0 / size as f64).round();
            self.tj_x += wk * self.size_bp / 1000.0;
        }
        width
    }

    /// A rule with lower left (h, v) going up `ht`.
    fn rule_out(&mut self, h: i32, v: i32, w: i32, ht: i32) {
        self.end_text();
        let x = self.xu(h);
        let y = self.yu(v);
        let wu = self.xu(h + w) - x;
        let hu = self.yu(v - ht) - y;
        let t = format!("{} {} {} {} re f\n", self.f(x), self.f(y), self.f(wu), self.f(hu));
        self.put(&t);
    }

    fn box_out(&mut self, b: &BoxNode, h: i32, base: i32) {
        if b.vertical {
            self.vlist_out(b, h, base);
        } else {
            self.hlist_out(b, h, base);
        }
    }

    /// tex.web 625/634: a glue's size with `cur_g`/`cur_glue` rounding.
    fn glue_amount(b: &BoxNode, g: &crate::val::Glue, cur_g: &mut i32, cur_glue: &mut f64) -> i32 {
        let mut w = g.width - *cur_g;
        if b.set.sign > 0 {
            if g.stretch_order == b.set.order {
                *cur_glue += g.stretch as f64;
                *cur_g = glue_round(b.set.ratio * *cur_glue);
            }
        } else if b.set.sign < 0 && g.shrink_order == b.set.order {
            *cur_glue -= g.shrink as f64;
            *cur_g = glue_round(b.set.ratio * *cur_glue);
        }
        w += *cur_g;
        w
    }

    fn hlist_out(&mut self, b: &BoxNode, left: i32, base: i32) {
        self.level += 1;
        let lvl = self.level;
        let me = Parent { vertical: false, left, base, width: b.width, height: b.height, depth: b.depth };
        // a link continued from the previous line starts at this box's left edge
        for l in &mut self.links {
            if l.level == lvl && l.seg.is_none() {
                l.seg = Some((left, base, b.height, b.depth));
            }
        }
        let mut cur_h = left;
        let mut cur_g = 0i32;
        let mut cur_glue = 0f64;
        for it in b.list.iter() {
            match it {
                Item::Char { font, c } | Item::Lig { font, c, .. } => {
                    cur_h += self.char_out(*font, *c, cur_h, base);
                }
                Item::Box(bx) => {
                    if !bx.list.is_empty() {
                        self.box_out(bx, cur_h, base + bx.shift);
                    }
                    cur_h += bx.width;
                }
                Item::Rule { w, h, d } => {
                    let ht = if *h == RUNNING { b.height } else { *h };
                    let dp = if *d == RUNNING { b.depth } else { *d };
                    if ht + dp > 0 && *w > 0 {
                        self.rule_out(cur_h, base + dp, *w, ht + dp);
                    }
                    cur_h += *w;
                }
                Item::Glue { g, sub, leader } => {
                    let mut rule_wd = Self::glue_amount(b, g, &mut cur_g, &mut cur_glue);
                    if *sub >= 100 {
                        if let Some(l) = leader {
                            match &**l {
                                Item::Rule { h, d, .. } => {
                                    let ht = if *h == RUNNING { b.height } else { *h };
                                    let dp = if *d == RUNNING { b.depth } else { *d };
                                    if ht + dp > 0 && rule_wd > 0 {
                                        self.rule_out(cur_h, base + dp, rule_wd, ht + dp);
                                    }
                                }
                                Item::Box(lb) => {
                                    let leader_wd = lb.width;
                                    if leader_wd > 0 && rule_wd > 0 {
                                        rule_wd += 10;
                                        let edge = cur_h + rule_wd;
                                        let mut lx = 0;
                                        if *sub == 100 {
                                            let save_h = cur_h;
                                            cur_h = left + leader_wd * ((cur_h - left) / leader_wd);
                                            if cur_h < save_h {
                                                cur_h += leader_wd;
                                            }
                                        } else {
                                            let lq = rule_wd / leader_wd;
                                            let lr = rule_wd % leader_wd;
                                            if *sub == 101 {
                                                cur_h += lr / 2;
                                            } else {
                                                lx = lr / (lq + 1);
                                                cur_h += (lr - (lq - 1) * lx) / 2;
                                            }
                                        }
                                        while cur_h + leader_wd <= edge {
                                            if !lb.list.is_empty() {
                                                self.box_out(lb, cur_h, base + lb.shift);
                                            }
                                            cur_h += leader_wd + lx;
                                        }
                                        cur_h = edge - 10;
                                        continue;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    cur_h += rule_wd;
                }
                Item::Kern { w, .. } | Item::MarginKern { w, .. } | Item::Math { w, .. } => cur_h += *w,
                Item::Whatsit(w) => {
                    if let Whatsit::Pdf(p) = &**w {
                        if let Some((ww, _, _)) = p.dims() {
                            self.pdf_what(p, cur_h, base, me);
                            cur_h += ww;
                            continue;
                        }
                        self.pdf_what(p, cur_h, base, me);
                    }
                }
                _ => {}
            }
        }
        // links still open end this segment at the box's right edge
        let right = left + b.width;
        let mut segs = Vec::new();
        for l in &mut self.links {
            if l.level == lvl {
                if let Some(seg) = l.seg.take() {
                    segs.push((l.num.take(), l.attr.clone(), l.action.clone(), l.h, l.d, seg));
                }
            }
        }
        for (num, attr, action, h, d, seg) in segs {
            self.link_segment(num, attr, action, h, d, seg, right);
        }
        self.level -= 1;
    }

    fn vlist_out(&mut self, b: &BoxNode, left: i32, base: i32) {
        self.level += 1;
        let lvl = self.level;
        let mut cur_v = base - b.height;
        let top = cur_v;
        if let Some(t) = self.carry.thread.as_mut() {
            if t.level == lvl && t.seg.is_none() {
                t.seg = Some((left, b.width, top));
            }
        }
        let mut cur_g = 0i32;
        let mut cur_glue = 0f64;
        for it in b.list.iter() {
            let me = Parent { vertical: true, left, base: cur_v, width: b.width, height: b.height, depth: b.depth };
            match it {
                Item::Box(bx) => {
                    if bx.list.is_empty() {
                        cur_v += bx.height + bx.depth;
                    } else {
                        cur_v += bx.height;
                        self.box_out(bx, left + bx.shift, cur_v);
                        cur_v += bx.depth;
                    }
                }
                Item::Rule { w, h, d } => {
                    let ht = h + d;
                    cur_v += ht;
                    let w = if *w == RUNNING { b.width } else { *w };
                    if ht > 0 && w > 0 {
                        self.rule_out(left, cur_v, w, ht);
                    }
                }
                Item::Glue { g, sub, leader } => {
                    let mut rule_ht = Self::glue_amount(b, g, &mut cur_g, &mut cur_glue);
                    if *sub >= 100 {
                        if let Some(l) = leader {
                            match &**l {
                                Item::Rule { w, .. } => {
                                    let w = if *w == RUNNING { b.width } else { *w };
                                    cur_v += rule_ht;
                                    if rule_ht > 0 && w > 0 {
                                        self.rule_out(left, cur_v, w, rule_ht);
                                    }
                                    continue;
                                }
                                Item::Box(lb) => {
                                    let leader_ht = lb.height + lb.depth;
                                    if leader_ht > 0 && rule_ht > 0 {
                                        rule_ht += 10;
                                        let edge = cur_v + rule_ht;
                                        let mut lx = 0;
                                        if *sub == 100 {
                                            let save_v = cur_v;
                                            cur_v = top + leader_ht * ((cur_v - top) / leader_ht);
                                            if cur_v < save_v {
                                                cur_v += leader_ht;
                                            }
                                        } else {
                                            let lq = rule_ht / leader_ht;
                                            let lr = rule_ht % leader_ht;
                                            if *sub == 101 {
                                                cur_v += lr / 2;
                                            } else {
                                                lx = lr / (lq + 1);
                                                cur_v += (lr - (lq - 1) * lx) / 2;
                                            }
                                        }
                                        while cur_v + leader_ht <= edge {
                                            let bl = cur_v + lb.height;
                                            if !lb.list.is_empty() {
                                                self.box_out(lb, left + lb.shift, bl);
                                            }
                                            cur_v = bl - lb.height + leader_ht + lx;
                                        }
                                        cur_v = edge - 10;
                                        continue;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    cur_v += rule_ht;
                }
                Item::Kern { w, .. } => cur_v += *w,
                Item::Whatsit(w) => {
                    if let Whatsit::Pdf(p) = &**w {
                        if let Some((_, hh, dd)) = p.dims() {
                            // a box-like whatsit: placed like a box
                            cur_v += hh;
                            self.pdf_what(p, left, cur_v, me);
                            cur_v += dd;
                            continue;
                        }
                        self.pdf_what(p, left, cur_v, me);
                    }
                }
                _ => {}
            }
        }
        // an open thread bead ends at this box's bottom
        let bottom = top + b.height + b.depth;
        let close = match self.carry.thread.as_mut() {
            Some(t) if t.level == lvl => t.seg.take().map(|seg| (t.id.clone(), t.attr.clone(), t.w, seg)),
            _ => None,
        };
        if let Some((id, attr, w, (l, bw, t))) = close {
            self.bead(id, attr, l, if w == RUNNING { bw } else { w }, t, bottom);
        }
        self.level -= 1;
    }

    fn bead(&mut self, id: Dk, attr: Option<Arc<str>>, left: i32, width: i32, top: i32, bottom: i32) {
        let m = self.p.thread_margin;
        let rect = self.rect(left - m, bottom + m, left + width + m, top - m);
        self.beads.push(BeadOut { id, rect, attr });
    }

    /// `[llx lly urx ury]` from TeX coordinates (h, v of lower left, upper right).
    fn rect(&self, h1: i32, v1: i32, h2: i32, v2: i32) -> String {
        format!("[{} {} {} {}]", self.f(self.xu(h1)), self.f(self.yu(v1)), self.f(self.xu(h2)), self.f(self.yu(v2)))
    }

    #[allow(clippy::too_many_arguments)]
    fn link_segment(
        &mut self,
        num: Option<u32>,
        attr: Option<Arc<str>>,
        action: Arc<Action>,
        h: i32,
        d: i32,
        seg: (i32, i32, i32, i32),
        end: i32,
    ) {
        let (sh, base, bh, bd) = seg;
        let ht = if h == RUNNING { bh } else { h };
        let dp = if d == RUNNING { bd } else { d };
        let m = self.p.link_margin;
        let rect = self.rect(sh - m, base + dp + m, end + m, base - ht - m);
        self.annots.push(AnnotOut::Link { num, rect, attr, action });
    }

    /// Rectangle of an annot-like node, running dimensions from its box.
    #[allow(clippy::too_many_arguments)]
    fn node_rect(&self, h: i32, v: i32, w: i32, ht: i32, d: i32, par: Parent, margin: i32) -> String {
        let right = if w == RUNNING { par.left + par.width } else { h + w };
        let (top, bot) = if par.vertical {
            (if ht == RUNNING { v } else { v - ht }, if d == RUNNING { v } else { v + d })
        } else {
            (
                if ht == RUNNING { par.base - par.height } else { v - ht },
                if d == RUNNING { par.base + par.depth } else { v + d },
            )
        };
        self.rect(h - margin, bot + margin, right + margin, top - margin)
    }

    fn literal(&mut self, mode: LiteralMode, data: &str, h: i32, v: i32) {
        match mode {
            LiteralMode::SetOrigin => {
                self.end_text();
                let (x, y) = (self.xu(h), self.yu(v));
                let t = format!(
                    "1 0 0 1 {} {} cm\n{data}\n1 0 0 1 {} {} cm\n",
                    self.f(x),
                    self.f(y),
                    self.f(-x),
                    self.f(-y)
                );
                self.put(&t);
            }
            LiteralMode::Page => {
                self.end_text();
                self.put(data);
                self.put("\n");
            }
            LiteralMode::Direct => {
                self.end_tj();
                self.put(data);
                self.put("\n");
            }
        }
    }

    fn pdf_what(&mut self, p: &PdfWhat, h: i32, v: i32, par: Parent) {
        match p {
            PdfWhat::Literal { mode, text, .. } => self.literal(*mode, text, h, v),
            PdfWhat::ColorStack { stack, op, text, init } => {
                let i = *stack as usize;
                while self.carry.colors.len() <= i {
                    self.carry.colors.push(Vec::new());
                    self.carry.page_mode.push(true);
                }
                if self.carry.colors[i].is_empty() {
                    self.carry.colors[i].push(init.literal.clone());
                    self.carry.page_mode[i] = init.page;
                }
                let st = &mut self.carry.colors[i];
                match op {
                    StackOp::Set => {
                        if let Some(t) = st.last_mut() {
                            *t = text.clone();
                        }
                    }
                    StackOp::Push => st.push(text.clone()),
                    StackOp::Pop => {
                        if st.len() > 1 {
                            st.pop();
                        } else {
                            self.warnings.push(format!("pdfTeX warning: pop empty color page stack {i}"));
                            return;
                        }
                    }
                    StackOp::Current => {}
                }
                let lit = self.carry.colors[i].last().cloned().unwrap_or_default();
                let mode = if init.direct { LiteralMode::Direct } else { LiteralMode::Page };
                self.literal(mode, &lit, h, v);
            }
            PdfWhat::SetMatrix(data) => {
                self.end_text();
                let n: Vec<f64> = data.split_whitespace().filter_map(|t| t.parse().ok()).collect();
                if n.len() < 4 {
                    self.warnings.push("pdfTeX warning: Unrecognized format of \\pdfsetmatrix".into());
                    return;
                }
                let (a, b, c, d) = (n[0], n[1], n[2], n[3]);
                let x = self.p.num.to_f(self.xu(h));
                let y = self.p.num.to_f(self.yu(v));
                let e = x - a * x - c * y;
                let f = y - b * x - d * y;
                let dd = self.p.num.digits as usize + 2;
                let t = format!(
                    "{} {} {} {} {} {} cm\n",
                    fmt_f(a, 6),
                    fmt_f(b, 6),
                    fmt_f(c, 6),
                    fmt_f(d, 6),
                    fmt_f(e, dd),
                    fmt_f(f, dd)
                );
                self.put(&t);
            }
            PdfWhat::Save => {
                self.end_text();
                self.save_depth += 1;
                self.put("q\n");
            }
            PdfWhat::Restore => {
                self.end_text();
                if self.save_depth == 0 {
                    self.warnings.push("pdfTeX warning: \\pdfrestore: missing \\pdfsave".into());
                } else {
                    self.save_depth -= 1;
                }
                self.put("Q\n");
            }
            PdfWhat::RefObj { id, obj } => {
                self.objs.insert(*id, obj.clone());
            }
            PdfWhat::RefXform { id, depth, form, .. } => {
                self.end_text();
                self.xforms.insert(*id);
                self.objs.insert(*id, form.clone());
                let t = format!("q\n1 0 0 1 {} {} cm\n/Fm{} Do\nQ\n", self.f(self.xu(h)), self.f(self.yu(v + depth)), id);
                self.put(&t);
            }
            PdfWhat::RefXimage { id, width, height, depth, image } => {
                self.end_text();
                self.images.insert(*id);
                self.objs.insert(*id, image.clone());
                let x = self.xu(h);
                let y = self.yu(v + depth);
                let info = match &**image {
                    PdfObj::Image(img) => Some(crate::image::info_with(
                        &img.bytes,
                        &crate::image::Request { page: img.page, page_box: img.pagebox as i32 },
                        &self.iparams,
                    )),
                    _ => None,
                };
                let t = match info {
                    Some(info) if info.kind == crate::image::Kind::Pdf => {
                        let nat_w = info.width.max(1) as f64;
                        let nat_h = (info.height + info.depth).max(1) as f64;
                        let sx = *width as f64 / nat_w;
                        let sy = (height + depth) as f64 / nat_h;
                        let ox = self.p.num.to_f(self.p.num.units(info.orig_x as i64)) * sx;
                        let oy = self.p.num.to_f(self.p.num.units(info.orig_y as i64)) * sy;
                        let dd = self.p.num.digits as usize + 2;
                        format!(
                            "q\n{} 0 0 {} {} {} cm\n/Im{} Do\nQ\n",
                            fmt_f(sx, 6),
                            fmt_f(sy, 6),
                            fmt_f(self.p.num.to_f(x) - ox, dd),
                            fmt_f(self.p.num.to_f(y) - oy, dd),
                            id
                        )
                    }
                    _ => {
                        let wu = self.xu(h + width) - x;
                        let hu = self.yu(v - height) - y;
                        format!("q\n{} 0 0 {} {} {} cm\n/Im{} Do\nQ\n", self.f(wu), self.f(hu), self.f(x), self.f(y), id)
                    }
                };
                self.put(&t);
            }
            PdfWhat::Annot { id, width, height, depth, data } => {
                let rect = self.node_rect(h, v, *width, *height, *depth, par, 0);
                self.annots.push(AnnotOut::Annot { num: *id, rect, data: data.clone() });
            }
            PdfWhat::StartLink { id, height, depth, attr, action, .. } => {
                if par.vertical {
                    self.warnings.push("! pdfTeX error (ext4): link annotations cannot be inside a vbox.".into());
                    return;
                }
                self.links.push(OpenLink {
                    num: Some(*id),
                    attr: attr.clone(),
                    action: action.clone(),
                    h: *height,
                    d: *depth,
                    level: self.level,
                    seg: Some((h, par.base, par.height, par.depth)),
                });
            }
            PdfWhat::EndLink => {
                let Some(mut l) = self.links.pop() else {
                    self.warnings
                        .push("! pdfTeX error (ext4): pdf_link_stack empty, \\pdfendlink used without \\pdfstartlink?".into());
                    return;
                };
                if let Some(seg) = l.seg.take() {
                    self.link_segment(l.num.take(), l.attr, l.action, l.h, l.d, seg, h);
                }
            }
            PdfWhat::Dest { id, kind, width, height, depth } => {
                let m = self.p.dest_margin;
                let x = self.f(self.xu(h - m));
                let y = self.f(self.yu(v - m));
                let spec = match kind {
                    DestKind::Xyz { zoom } => {
                        let z = match zoom {
                            Some(z) => fmt_f(*z as f64 / 1000.0, 3),
                            None => "null".into(),
                        };
                        format!("/XYZ {x} {y} {z}")
                    }
                    DestKind::Fit => "/Fit".into(),
                    DestKind::FitB => "/FitB".into(),
                    DestKind::FitH => format!("/FitH {y}"),
                    DestKind::FitBH => format!("/FitBH {y}"),
                    DestKind::FitV => format!("/FitV {x}"),
                    DestKind::FitBV => format!("/FitBV {x}"),
                    DestKind::FitR => {
                        let r = self.node_rect(h, v, *width, *height, *depth, par, m);
                        format!("/FitR {}", &r[1..r.len() - 1])
                    }
                };
                self.dests.push((dk(id), spec));
            }
            PdfWhat::Thread { start: false, id, attr, width, height, depth } => {
                let rect = self.node_rect(h, v, *width, *height, *depth, par, self.p.thread_margin);
                self.beads.push(BeadOut { id: dk(id), rect, attr: attr.clone() });
            }
            PdfWhat::Thread { start: true, id, attr, width, depth, .. } => {
                let level = if par.vertical { self.level } else { self.level.saturating_sub(1) };
                let w = if *width == RUNNING { par.width } else { *width };
                self.carry.thread =
                    Some(OpenThread { id: dk(id), attr: attr.clone(), w: *width, d: *depth, level, seg: Some((par.left, w, v)) });
            }
            PdfWhat::EndThread => match self.carry.thread.take() {
                Some(t) => {
                    if let Some((l, bw, top)) = t.seg {
                        let bottom = if t.d == RUNNING { v } else { v + t.d };
                        self.bead(t.id, t.attr, l, if t.w == RUNNING { bw } else { t.w }, top, bottom);
                    }
                }
                None => self.warnings.push(
                    "! pdfTeX error (ext4): \\pdfendthread ended up in different nesting level than \\pdfstartthread.".into(),
                ),
            },
            PdfWhat::SavePos => {
                let x = h + self.p.h_off;
                let y = self.p.page_h - v - self.p.v_off;
                self.saved.push((x, y));
            }
        }
    }

    /// pdfTeX's colour stack page start: current values of page stacks
    /// that differ from their initial value.
    fn page_start(&mut self, inits: &[StackInit]) {
        for (i, st) in self.carry.colors.clone().iter().enumerate() {
            let page = self.carry.page_mode.get(i).copied().unwrap_or(true);
            if !page {
                continue;
            }
            if let Some(top) = st.last() {
                let init = inits.get(i).map(|s| s.literal.clone());
                if init.as_deref() != Some(&**top) {
                    self.put(top);
                    self.put("\n");
                }
            }
        }
    }

    fn finish(mut self, width: i32, height: i32, level: i32) -> PageOut {
        self.end_text();
        for _ in 0..self.save_depth {
            self.put("Q\n");
        }
        if self.save_depth > 0 {
            self.warnings.push("pdfTeX warning: \\pdfsave without \\pdfrestore".into());
        }
        if !self.links.is_empty() {
            self.warnings.push("pdfTeX warning: link annotation still open at the end of the page".into());
        }
        let (stream, filtered) = encode(&self.s, level);
        PageOut {
            stream,
            filtered,
            fonts: self.fonts,
            glyphs: self.glyphs,
            xforms: self.xforms,
            images: self.images,
            annots: self.annots,
            dests: self.dests,
            beads: self.beads,
            saved: self.saved,
            warnings: self.warnings,
            carry_out: self.carry,
            objs: self.objs,
            width,
            height,
        }
    }
}

fn initial_carry(inits: &[StackInit]) -> Carry {
    Carry {
        colors: inits.iter().map(|s| vec![s.literal.clone()]).collect(),
        page_mode: inits.iter().map(|s| s.page).collect(),
        thread: None,
    }
}

// ------------------------------------------------------------------ the incremental writer

struct CachedPage {
    shipped: Arc<Shipped>,
    carry_in: Carry,
    level: i32,
    iparams: crate::image::Params,
    out: Arc<PageOut>,
}

struct CachedXForm {
    b: Arc<BoxNode>,
    level: i32,
    num: Num,
    out: Arc<PageOut>,
}

/// The PDF of a job, kept across builds.
#[derive(Default)]
pub struct Pdf {
    pages: Vec<Option<CachedPage>>,
    xforms: HashMap<u32, CachedXForm>,
    /// Pages rendered (not reused) by the last `write`.
    pub rebuilt: usize,
}

fn new_window(n: Option<bool>) -> &'static str {
    match n {
        Some(true) => " /NewWindow true",
        Some(false) => " /NewWindow false",
        None => "",
    }
}

/// An action's dictionary (`<< ... >>`), or the user text as given.
fn action_text(
    a: &Action,
    page_objs: &[u32],
    dest_obj: &mut dyn FnMut(&Dk) -> u32,
    thread_obj: &mut dyn FnMut(&Dk) -> u32,
) -> String {
    match a {
        Action::User(t) => t.to_string(),
        Action::Goto { file: None, target, .. } => match target {
            GotoTarget::Name(n) => format!("<< /S /GoTo /D ({n}) >>"),
            GotoTarget::Num(n) => format!("<< /S /GoTo /D {} 0 R >>", dest_obj(&Dk::Num(*n))),
            GotoTarget::Page { page, spec } => {
                let p = page_objs.get((*page - 1).max(0) as usize).copied().unwrap_or(0);
                format!("<< /S /GoTo /D [{p} 0 R {spec}] >>")
            }
        },
        Action::Goto { file: Some(f), target, new_window: nw } => {
            let d = match target {
                GotoTarget::Name(n) => format!("({n})"),
                GotoTarget::Num(n) => format!("[{} /Fit]", n - 1),
                GotoTarget::Page { page, spec } => format!("[{} {}]", page - 1, spec),
            };
            format!("<< /S /GoToR /F ({f}) /D {d}{} >>", new_window(*nw))
        }
        Action::Thread { file, id, new_window: nw } => match file {
            Some(f) => {
                let d = match id {
                    DestId::Name(n) => format!("({n})"),
                    DestId::Num(n) => format!("{n}"),
                };
                format!("<< /S /Thread /F ({f}) /D {d}{} >>", new_window(*nw))
            }
            None => format!("<< /S /Thread /D {} 0 R >>", thread_obj(&dk(id))),
        },
    }
}

struct ONode {
    i: usize,
    kids: Vec<ONode>,
}

/// pdfTeX's outline order: an entry with `count n` has the next |n| entries
/// (each with its own subtree) as children.
fn outline_tree(items: &[crate::pdfprim::Outline], i: &mut usize, n: Option<usize>) -> Vec<ONode> {
    let mut v = Vec::new();
    while *i < items.len() && n.is_none_or(|n| v.len() < n) {
        let me = *i;
        *i += 1;
        let k = items[me].count.unsigned_abs() as usize;
        let kids = if k > 0 { outline_tree(items, i, Some(k)) } else { Vec::new() };
        v.push(ONode { i: me, kids });
    }
    v
}

fn visible(items: &[crate::pdfprim::Outline], n: &ONode) -> i64 {
    if items[n.i].count > 0 {
        n.kids.len() as i64 + n.kids.iter().map(|k| visible(items, k)).sum::<i64>()
    } else {
        0
    }
}

fn alloc_outlines(w: &mut Writer, n: &[ONode], nums: &mut HashMap<usize, u32>) {
    for k in n {
        nums.insert(k.i, w.alloc());
        alloc_outlines(w, &k.kids, nums);
    }
}

fn image_params(gp: &[(Key, Val)], digits: i32) -> crate::image::Params {
    let mut p = crate::image::Params::default();
    p.pdf_minor = ipar(gp, "pdfminorversion", p.pdf_minor);
    p.compress_level = ipar(gp, "pdfcompresslevel", p.compress_level);
    p.image_resolution = ipar(gp, "pdfimageresolution", p.image_resolution);
    p.page_box = ipar(gp, "pdfpagebox", p.page_box);
    p.inclusion_errorlevel = ipar(gp, "pdfinclusionerrorlevel", p.inclusion_errorlevel);
    p.inclusion_copy_fonts = ipar(gp, "pdfinclusioncopyfonts", p.inclusion_copy_fonts);
    p.gamma = ipar(gp, "pdfgamma", p.gamma);
    p.image_gamma = ipar(gp, "pdfimagegamma", p.image_gamma);
    p.image_hicolor = ipar(gp, "pdfimagehicolor", p.image_hicolor);
    p.image_apply_gamma = ipar(gp, "pdfimageapplygamma", p.image_apply_gamma);
    p.suppress_ptex_info = ipar(gp, "pdfsuppressptexinfo", p.suppress_ptex_info);
    p.decimal_digits = digits;
    p
}

/// A page's or form's `/Resources` dictionary.
fn resources(
    w: &mut Writer,
    fs: &mut crate::pdffont::FontSet,
    font_objs: &mut BTreeMap<FontId, u32>,
    o: &PageOut,
    extra: &str,
    image_objs: &BTreeMap<u32, u32>,
    omit_procset: bool,
) -> String {
    let mut r = String::from("<<\n");
    if !o.fonts.is_empty() {
        r.push_str("/Font <<");
        for f in &o.fonts {
            let n = match font_objs.get(f) {
                Some(&n) => n,
                None => {
                    let n = fs.resource(w, *f, 0).1;
                    font_objs.insert(*f, n);
                    n
                }
            };
            let _ = write!(r, " /F{f} {n} 0 R");
        }
        r.push_str(" >>\n");
    }
    if !o.xforms.is_empty() || !o.images.is_empty() {
        r.push_str("/XObject <<");
        for x in &o.xforms {
            let _ = write!(r, " /Fm{x} {x} 0 R");
        }
        for i in &o.images {
            if let Some(n) = image_objs.get(i) {
                let _ = write!(r, " /Im{i} {n} 0 R");
            }
        }
        r.push_str(" >>\n");
    }
    if !omit_procset {
        r.push_str("/ProcSet [ /PDF /Text");
        if !o.images.is_empty() {
            r.push_str(" /ImageB /ImageC /ImageI");
        }
        r.push_str(" ]\n");
    }
    if !extra.is_empty() {
        r.push_str(extra);
        r.push('\n');
    }
    r.push_str(">>");
    r
}

impl Pdf {
    pub fn new() -> Pdf {
        Pdf::default()
    }

    fn render_page(sh: &Shipped, carry: Carry, level: i32, iparams: crate::image::Params, inits: &[StackInit]) -> PageOut {
        let pp = page_pars(&sh.params, &sh.b);
        let mut r = R::new(pp, iparams, carry);
        r.page_start(inits);
        r.box_out(&sh.b, 0, sh.b.height);
        r.finish(pp.page_w, pp.page_h, level)
    }

    fn render_xform(b: &BoxNode, num: Num, level: i32, iparams: crate::image::Params, inits: &[StackInit]) -> PageOut {
        let h = b.height + b.depth;
        let pp = PagePars { num, h_off: 0, v_off: 0, page_w: b.width, page_h: h, link_margin: 0, dest_margin: 0, thread_margin: 0 };
        let mut r = R::new(pp, iparams, initial_carry(inits));
        r.box_out(b, 0, b.height);
        r.finish(b.width, h, level)
    }

    /// Writes the whole file, reusing every page whose inputs are unchanged.
    pub fn write(&mut self, job: &Job) -> Built {
        let gp = job.params;
        let pj = job.pdf;
        let level = ipar(gp, "pdfcompresslevel", 9).clamp(0, 9);
        let minor = ipar(gp, "pdfminorversion", 5);
        let objc = ipar(gp, "pdfobjcompresslevel", 0);
        let digits = ipar(gp, "pdfdecimaldigits", 3).clamp(0, 4) as u32;
        let gnum = Num { digits, mag: ipar(gp, "mag", 1000).max(1) as i64 };
        let iparams = image_params(gp, digits as i32);
        let omit_procset = ipar(gp, "pdfomitprocset", 0) > 0;
        let inits: Vec<StackInit> = if pj.colorstacks.is_empty() {
            vec![StackInit { page: true, direct: false, literal: "0 g 0 G".into() }]
        } else {
            pj.colorstacks.clone()
        };
        let mut built = Built::default();
        let mut w = Writer::new(minor, level, objc, pj.ids + 1);
        let mut fs = crate::pdffont::FontSet::new(job.loader.clone());

        // ---- page contents (rendered or reused)
        self.pages.truncate(job.pages.len());
        self.pages.resize_with(job.pages.len(), || None);
        self.rebuilt = 0;
        let mut carry = initial_carry(&inits);
        let mut outs: Vec<Arc<PageOut>> = Vec::with_capacity(job.pages.len());
        for (i, sh) in job.pages.iter().enumerate() {
            let cached = self.pages[i].as_ref().and_then(|c| {
                let same = (Arc::ptr_eq(&c.shipped, sh) || *c.shipped == **sh)
                    && c.carry_in == carry
                    && c.level == level
                    && c.iparams == iparams;
                same.then(|| c.out.clone())
            });
            let out = match cached {
                Some(o) => o,
                None => {
                    self.rebuilt += 1;
                    let o = Arc::new(Self::render_page(sh, carry.clone(), level, iparams, &inits));
                    self.pages[i] =
                        Some(CachedPage { shipped: sh.clone(), carry_in: carry.clone(), level, iparams, out: o.clone() });
                    o
                }
            };
            carry = out.carry_out.clone();
            outs.push(out);
        }
        if carry.thread.is_some() {
            built.warnings.push("pdfTeX warning: \\pdfstartthread without \\pdfendthread".into());
        }

        // ---- every object: those the job made (immediate or not) and those pages reference
        let mut objs: BTreeMap<u32, Arc<PdfObj>> = BTreeMap::new();
        for (id, o) in &pj.objs {
            objs.insert(*id, o.clone());
        }
        for o in &outs {
            for (id, ob) in &o.objs {
                objs.entry(*id).or_insert_with(|| ob.clone());
            }
        }

        // xform contents (xforms inside xforms are found as we go)
        let mut xouts: Vec<(u32, Arc<str>, Arc<str>, Arc<BoxNode>, Arc<PageOut>)> = Vec::new();
        let mut done: BTreeSet<u32> = BTreeSet::new();
        let mut todo: Vec<u32> = objs.iter().filter(|(_, o)| matches!(&***o, PdfObj::Xform { .. })).map(|(i, _)| *i).collect();
        let mut keep: BTreeSet<u32> = BTreeSet::new();
        while let Some(id) = todo.pop() {
            if !done.insert(id) {
                continue;
            }
            let Some(PdfObj::Xform { attr, resources, b, .. }) = objs.get(&id).map(|o| (**o).clone()) else { continue };
            keep.insert(id);
            let ok = self.xforms.get(&id).is_some_and(|c| (Arc::ptr_eq(&c.b, &b) || *c.b == *b) && c.level == level && c.num == gnum);
            if !ok {
                let o = Arc::new(Self::render_xform(&b, gnum, level, iparams, &inits));
                self.xforms.insert(id, CachedXForm { b: b.clone(), level, num: gnum, out: o });
            }
            let o = self.xforms[&id].out.clone();
            for (k, ob) in &o.objs {
                if !objs.contains_key(k) {
                    objs.insert(*k, ob.clone());
                }
                if matches!(&**ob, PdfObj::Xform { .. }) {
                    todo.push(*k);
                }
            }
            xouts.push((id, attr.unwrap_or_else(|| "".into()), resources.unwrap_or_else(|| "".into()), b, o));
        }
        self.xforms.retain(|n, _| keep.contains(n));

        // ---- numbers
        let pages_root = w.alloc();
        let page_objs: Vec<u32> = (0..job.pages.len()).map(|_| w.alloc()).collect();
        let mut dest_nums: BTreeMap<Dk, u32> = BTreeMap::new();
        let mut thread_nums: BTreeMap<Dk, u32> = BTreeMap::new();
        let mut font_objs: BTreeMap<FontId, u32> = BTreeMap::new();
        let mut image_objs: BTreeMap<u32, u32> = BTreeMap::new();

        // ---- images (E6), each at its own id when E6 takes it
        for (id, o) in &objs {
            if let PdfObj::Image(img) = &**o {
                let req = crate::image::Request { page: img.page, page_box: img.pagebox as i32 };
                let mut sink = ImageSink { w: &mut w, first: Some(*id) };
                match crate::image::include(&mut sink, &img.name, &img.bytes, &req, &iparams) {
                    Ok(n) => {
                        image_objs.insert(*id, n);
                    }
                    Err(e) => built.warnings.push(format!("! pdfTeX error (ext1): {e}")),
                }
            }
        }

        // ---- glyphs
        for o in outs.iter().chain(xouts.iter().map(|x| &x.4)) {
            for (f, c) in &o.glyphs {
                fs.use_glyph(&mut w, *f, 0, *c);
            }
        }

        // ---- dests and beads, by page
        let mut beads_by_thread: BTreeMap<Dk, Vec<(u32, u32, String, Option<Arc<str>>)>> = BTreeMap::new();
        let mut page_beads: Vec<Vec<u32>> = vec![Vec::new(); outs.len()];
        let mut dest_specs: BTreeMap<Dk, String> = BTreeMap::new();
        let suppress_dup = ipar(gp, "pdfsuppresswarningdupdest", 0) > 0;
        for (i, o) in outs.iter().enumerate() {
            built.warnings.extend(o.warnings.iter().cloned());
            built.saved.push(o.saved.clone());
            let pnum = page_objs[i];
            for (id, spec) in &o.dests {
                if dest_specs.contains_key(id) {
                    if !suppress_dup {
                        built.warnings.push(format!(
                            "pdfTeX warning (ext4): destination with the same identifier ({}) has been already used, duplicate ignored",
                            dest_label(id)
                        ));
                    }
                    continue;
                }
                dest_specs.insert(id.clone(), format!("[{pnum} 0 R {spec}]"));
            }
            for b in &o.beads {
                let n = w.alloc();
                page_beads[i].push(n);
                beads_by_thread.entry(b.id.clone()).or_default().push((n, pnum, b.rect.clone(), b.attr.clone()));
            }
        }
        for id in dest_specs.keys() {
            let n = w.alloc();
            dest_nums.insert(id.clone(), n);
        }
        for id in beads_by_thread.keys() {
            let n = w.alloc();
            thread_nums.insert(id.clone(), n);
        }

        // ---- pages
        for (i, o) in outs.iter().enumerate() {
            let sh = &job.pages[i];
            let pnum = page_objs[i];
            let content = w.alloc();
            w.write_encoded(content, "", &o.stream, o.filtered);
            let mut annots = Vec::new();
            for a in &o.annots {
                match a {
                    AnnotOut::Annot { num, rect, data } => {
                        w.write_obj(*num, format!("<<\n/Type /Annot\n{data}\n/Rect {rect}\n>>").as_bytes());
                        annots.push(*num);
                    }
                    AnnotOut::Link { num, rect, attr, action } => {
                        let n = match num {
                            Some(n) => *n,
                            None => w.alloc(),
                        };
                        let attr = attr.as_deref().unwrap_or("");
                        let body = match &**action {
                            Action::User(t) => format!("<<\n/Type /Annot\n{attr}\n/Rect {rect}\n{t}\n>>"),
                            a => {
                                let mut extra: Vec<Dk> = Vec::new();
                                let at = {
                                    let mut dn = |id: &Dk| match dest_nums.get(id) {
                                        Some(&x) => x,
                                        None => {
                                            extra.push(id.clone());
                                            0
                                        }
                                    };
                                    let mut tn = |id: &Dk| thread_nums.get(id).copied().unwrap_or(0);
                                    action_text(a, &page_objs, &mut dn, &mut tn)
                                };
                                let at = if extra.is_empty() {
                                    at
                                } else {
                                    for id in extra {
                                        let x = w.alloc();
                                        dest_nums.insert(id, x);
                                    }
                                    let mut dn = |id: &Dk| dest_nums.get(id).copied().unwrap_or(0);
                                    let mut tn = |id: &Dk| thread_nums.get(id).copied().unwrap_or(0);
                                    action_text(a, &page_objs, &mut dn, &mut tn)
                                };
                                format!("<<\n/Type /Annot\n/Subtype /Link\n{attr}\n/Rect {rect}\n/A {at}\n>>")
                            }
                        };
                        w.write_obj(n, body.as_bytes());
                        annots.push(n);
                    }
                }
            }
            let res = resources(
                &mut w,
                &mut fs,
                &mut font_objs,
                o,
                &tpar(&sh.params, "pdfpageresources", job.names),
                &image_objs,
                omit_procset,
            );
            let attr = tpar(&sh.params, "pdfpageattr", job.names);
            let mut d = format!("<<\n/Type /Page\n/Contents {content} 0 R\n/Resources {res}\n");
            if !attr.contains("/MediaBox") {
                let n = page_pars(&sh.params, &sh.b).num;
                let _ = writeln!(d, "/MediaBox [0 0 {} {}]", n.bp(o.width as i64), n.bp(o.height as i64));
            }
            let _ = writeln!(d, "/Parent {pages_root} 0 R");
            if !annots.is_empty() {
                d.push_str("/Annots [");
                for a in &annots {
                    let _ = write!(d, " {a} 0 R");
                }
                d.push_str(" ]\n");
            }
            if !page_beads[i].is_empty() {
                d.push_str("/B [");
                for b in &page_beads[i] {
                    let _ = write!(d, " {b} 0 R");
                }
                d.push_str(" ]\n");
            }
            if !attr.is_empty() {
                d.push_str(&attr);
                d.push('\n');
            }
            d.push_str(">>");
            w.write_obj(pnum, d.as_bytes());
        }
        built.page_objs = page_objs.clone();

        // ---- xforms
        for (id, attr, res_extra, b, o) in &xouts {
            let res = resources(&mut w, &mut fs, &mut font_objs, o, res_extra, &image_objs, omit_procset);
            let h = b.height + b.depth;
            let mut dict = format!(
                "/Type /XObject\n/Subtype /Form\n/BBox [0 0 {} {}]\n/FormType 1\n/Matrix [1 0 0 1 0 0]\n/Resources {res}",
                gnum.bp(b.width as i64),
                gnum.bp(h as i64)
            );
            if !attr.is_empty() {
                dict.push('\n');
                dict.push_str(attr);
            }
            w.write_encoded(*id, &dict, &o.stream, o.filtered);
        }

        // ---- user objects
        for (id, o) in &objs {
            if let PdfObj::Obj { stream, attr, data, .. } = &**o {
                let bytes: &[u8] = match data {
                    ObjData::Text(t) => t.as_bytes(),
                    ObjData::File { bytes, .. } => bytes,
                };
                if *stream {
                    w.write_stream(*id, attr.as_deref().unwrap_or(""), bytes);
                } else {
                    w.write_obj(*id, bytes);
                }
            }
        }

        // ---- threads and beads
        let mut threads = Vec::new();
        for (id, beads) in &beads_by_thread {
            let t = thread_nums[id];
            threads.push(t);
            let k = beads.len();
            for (j, (n, pg, rect, _)) in beads.iter().enumerate() {
                let next = beads[(j + 1) % k].0;
                let prev = beads[(j + k - 1) % k].0;
                w.write_obj(*n, format!("<< /T {t} 0 R /V {prev} 0 R /N {next} 0 R /P {pg} 0 R /R {rect} >>").as_bytes());
            }
            let attr = beads.iter().find_map(|b| b.3.clone());
            let label = match id {
                Dk::Name(n) => n.to_string(),
                Dk::Num(n) => n.to_string(),
            };
            let info = match attr {
                Some(a) if a.contains("/Title") => a.to_string(),
                Some(a) => format!("/Title ({label}) {a}"),
                None => format!("/Title ({label})"),
            };
            w.write_obj(t, format!("<< /Type /Thread /I << {info} >> /F {} 0 R >>", beads[0].0).as_bytes());
        }

        // ---- outlines
        let outlines = &pj.outlines;
        let mut outlines_obj = None;
        if !outlines.is_empty() {
            let mut i = 0;
            let tree = outline_tree(outlines, &mut i, None);
            let root = w.alloc();
            outlines_obj = Some(root);
            let mut nums = HashMap::new();
            alloc_outlines(&mut w, &tree, &mut nums);
            let mut stack: Vec<(&Vec<ONode>, u32)> = vec![(&tree, root)];
            while let Some((kids, parent)) = stack.pop() {
                for (j, k) in kids.iter().enumerate() {
                    let it = &outlines[k.i];
                    let me = nums[&k.i];
                    // dests referenced but not (yet) numbered get numbers
                    if let Action::Goto { file: None, target: GotoTarget::Num(n), .. } = &it.action {
                        if !dest_nums.contains_key(&Dk::Num(*n)) {
                            let x = w.alloc();
                            dest_nums.insert(Dk::Num(*n), x);
                        }
                    }
                    let act = {
                        let mut dn = |id: &Dk| dest_nums.get(id).copied().unwrap_or(0);
                        let mut tn = |id: &Dk| thread_nums.get(id).copied().unwrap_or(0);
                        action_text(&it.action, &page_objs, &mut dn, &mut tn)
                    };
                    let mut d = format!("<<\n/Title ({})\n", it.title);
                    match &it.action {
                        Action::User(t) => {
                            let _ = writeln!(d, "{t}");
                        }
                        _ => {
                            let _ = writeln!(d, "/A {act}");
                        }
                    }
                    let _ = writeln!(d, "/Parent {parent} 0 R");
                    if j > 0 {
                        let _ = writeln!(d, "/Prev {} 0 R", nums[&kids[j - 1].i]);
                    }
                    if j + 1 < kids.len() {
                        let _ = writeln!(d, "/Next {} 0 R", nums[&kids[j + 1].i]);
                    }
                    if !k.kids.is_empty() {
                        let _ =
                            writeln!(d, "/First {} 0 R\n/Last {} 0 R", nums[&k.kids[0].i], nums[&k.kids[k.kids.len() - 1].i]);
                        let c = if it.count > 0 { visible(outlines, k) } else { -(k.kids.len() as i64) };
                        let _ = writeln!(d, "/Count {c}");
                    }
                    if let Some(a) = &it.attr {
                        d.push_str(a);
                        d.push('\n');
                    }
                    d.push_str(">>");
                    w.write_obj(me, d.as_bytes());
                    if !k.kids.is_empty() {
                        stack.push((&k.kids, me));
                    }
                }
            }
            let total = tree.len() as i64 + tree.iter().map(|k| visible(outlines, k)).sum::<i64>();
            let body = format!(
                "<<\n/Type /Outlines\n/First {} 0 R\n/Last {} 0 R\n/Count {total}\n>>",
                nums[&tree[0].i],
                nums[&tree[tree.len() - 1].i]
            );
            w.write_obj(root, body.as_bytes());
        }

        // ---- the open action may name a numbered dest too
        if let Some(Action::Goto { file: None, target: GotoTarget::Num(n), .. }) = &pj.open_action {
            if !dest_nums.contains_key(&Dk::Num(*n)) {
                let x = w.alloc();
                dest_nums.insert(Dk::Num(*n), x);
            }
        }

        // ---- dest objects
        for (id, n) in &dest_nums {
            match dest_specs.get(id) {
                Some(s) => w.write_obj(*n, s.as_bytes()),
                None => {
                    built.warnings.push(format!(
                        "pdfTeX warning (dest): {} has been referenced but does not exist, replaced by a fixed one",
                        dest_label(id)
                    ));
                    let first = page_objs.first().copied().unwrap_or(pages_root);
                    w.write_obj(*n, format!("[{first} 0 R /Fit]").as_bytes());
                }
            }
        }

        // ---- names (named dests are only reached through the name tree)
        let mut named: Vec<(Arc<str>, u32)> = dest_specs
            .keys()
            .filter_map(|id| match id {
                Dk::Name(s) => Some((s.clone(), dest_nums[id])),
                Dk::Num(_) => None,
            })
            .collect();
        named.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        let mut names_obj = None;
        if !named.is_empty() || !pj.names.is_empty() {
            let mut d = String::from("<<\n");
            if !named.is_empty() {
                let tree = w.alloc();
                let mut t = String::from("<<\n/Names [");
                for (s, n) in &named {
                    let _ = write!(t, " ({s}) {n} 0 R");
                }
                t.push_str(" ]\n>>");
                w.write_obj(tree, t.as_bytes());
                let _ = writeln!(d, "/Dests {tree} 0 R");
            }
            if !pj.names.is_empty() {
                d.push_str(&pj.names);
                d.push('\n');
            }
            d.push_str(">>");
            let n = w.alloc();
            w.write_obj(n, d.as_bytes());
            names_obj = Some(n);
        }

        // ---- fonts (E5)
        {
            let tag = crate::prim::tag::PDF;
            let none = Val::Void;
            let ops = pget(gp, &Key::Other(tag, crate::pdffont::MAP_OPS)).unwrap_or(&none);
            let gu = pget(gp, &Key::Other(tag, crate::pdffont::GLYPH_UNI)).unwrap_or(&none);
            let (map_ops, glyph_uni) = crate::pdffont::WriteOpts::from_vals(ops, gu);
            let fo = crate::pdffont::WriteOpts {
                compress_level: level,
                gen_tounicode: ipar(gp, "pdfgentounicode", 0) > 0,
                omit_charset: ipar(gp, "pdfomitcharset", 0) > 0,
                map_ops,
                glyph_uni,
            };
            fs.write(&mut w, &fo);
            built.warnings.extend(fs.take_warnings());
        }

        // ---- pages tree
        {
            let mut d = format!("<<\n/Type /Pages\n/Count {}\n/Kids [", page_objs.len());
            for p in &page_objs {
                let _ = write!(d, " {p} 0 R");
            }
            d.push_str(" ]\n");
            let pa = tpar(gp, "pdfpagesattr", job.names);
            if !pa.is_empty() {
                d.push_str(&pa);
                d.push('\n');
            }
            d.push_str(">>");
            w.write_obj(pages_root, d.as_bytes());
        }

        // ---- catalog
        let catalog = w.alloc();
        {
            let mut d = format!("<<\n/Type /Catalog\n/Pages {pages_root} 0 R\n");
            if let Some(o) = outlines_obj {
                let _ = writeln!(d, "/Outlines {o} 0 R");
            }
            if !threads.is_empty() {
                d.push_str("/Threads [");
                for t in &threads {
                    let _ = write!(d, " {t} 0 R");
                }
                d.push_str(" ]\n");
            }
            if let Some(n) = names_obj {
                let _ = writeln!(d, "/Names {n} 0 R");
            }
            if let Some(a) = &pj.open_action {
                let mut dn = |id: &Dk| dest_nums.get(id).copied().unwrap_or(0);
                let mut tn = |id: &Dk| thread_nums.get(id).copied().unwrap_or(0);
                let _ = writeln!(d, "/OpenAction {}", action_text(a, &page_objs, &mut dn, &mut tn));
            }
            if !pj.catalog.is_empty() {
                d.push_str(&pj.catalog);
                d.push('\n');
            }
            d.push_str(">>");
            w.write_obj(catalog, d.as_bytes());
        }

        // ---- info
        let info = w.alloc();
        {
            let user = &*pj.info;
            let mut d = String::from("<<\n");
            if !user.contains("/Producer") {
                d.push_str("/Producer (pdfTeX-1.40.26)\n");
            }
            if !user.contains("/Creator") {
                d.push_str("/Creator (TeX)\n");
            }
            if ipar(gp, "pdfinfoomitdate", 0) <= 0 && !job.creation_date.is_empty() {
                if !user.contains("/CreationDate") {
                    let _ = writeln!(d, "/CreationDate ({})", job.creation_date);
                }
                if !user.contains("/ModDate") {
                    let _ = writeln!(d, "/ModDate ({})", job.creation_date);
                }
            }
            if !user.contains("/Trapped") {
                d.push_str("/Trapped /False\n");
            }
            if ipar(gp, "pdfsuppressptexinfo", 0) & 1 == 0 {
                d.push_str(
                    "/PTEX.Fullbanner (This is pdfTeX, Version 3.141592653-2.6-1.40.26 (TeX Live 2026) kpathsea version 6.4.1)\n",
                );
            }
            if !user.is_empty() {
                d.push_str(user);
                d.push('\n');
            }
            d.push_str(">>");
            w.write_obj(info, d.as_bytes());
        }

        let md5 = phitex_codecs::md5::md5(&job.id_seed);
        let mut id = String::new();
        for b in md5 {
            let _ = write!(id, "{b:02X}");
        }
        built.bytes = w.finish(catalog, info, &id, &pj.trailer);
        built
    }
}

/// One-shot: the PDF of a job with no previous build.
pub fn write(job: &Job) -> Built {
    Pdf::new().write(job)
}
