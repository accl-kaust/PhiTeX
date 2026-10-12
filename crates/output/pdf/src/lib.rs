//! pdfTeX's PDF backend: pages from shipped boxes, Type 1 fonts embedded
//! and subset; byte-identical with pdfTeX 1.40.29.
//!
//! [`PdfWriter`] is `pdf_ship_out` (pdfTeX §750–§771) with
//! `pdf_hlist_out` and `pdf_vlist_out` (pdfTeX §729–§746) over
//! [`phitex_typeset`] nodes, the text state machine of pdfTeX §691–§693,
//! the object table of pdfTeX §694–§702, the fonts of `writefont.c`,
//! `writet1.c` and `writeenc.c`, and the end of the file (pdfTeX
//! §794–§815). Streams are compressed by `phitex_codecs`' port of zlib,
//! the library pdfTeX links.
//!
//! Not in this crate yet: images, links and annotations, `\pdfobj` and
//! forms, object streams (`\pdfobjcompresslevel>0`), TrueType, OpenType,
//! PK and virtual fonts, font expansion, `SlantFont`/`ExtendFont`.

#![allow(
    clippy::unreadable_literal,
    clippy::many_single_char_names,
    clippy::similar_names
)]

pub mod enc;
pub mod map;
pub mod num;
pub mod t1;

use std::collections::{BTreeMap, BTreeSet};

use phitex_tfm::ScaledFont;
use phitex_typeset::arith::Scaled;
use phitex_typeset::fonts::FontId;
use phitex_typeset::glue::{GlueSign, Order};
use phitex_typeset::node::{BoxKind, BoxNode, GlueKind, GlueNode, Node, RUNNING};

use crate::enc::GlyphNames;
use crate::map::MapEntry;
use crate::num::{ONE_BP, ONE_HUNDRED_BP, ONE_HUNDRED_INCH, TEN_POW, divide_scaled};
use crate::t1::{FontDesc, dim};

/// Why output failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PdfError {
    /// "Huge page cannot be shipped out" (tex.web §641, pdfTeX §751).
    HugePage,
    /// A font that cannot be written (a fatal `pdftex_fail` in pdfTeX).
    Font(String),
    /// Something this crate does not do yet.
    Unsupported(String),
}

impl std::fmt::Display for PdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PdfError::HugePage => write!(f, "Huge page cannot be shipped out"),
            PdfError::Font(m) => write!(f, "pdfTeX error (font): {m}"),
            PdfError::Unsupported(m) => write!(f, "not supported: {m}"),
        }
    }
}

impl std::error::Error for PdfError {}

/// The parameters pdfTeX reads when it writes: `\pdf...` registers,
/// `\hoffset`, `\voffset`, `\mag`, and what the system gives (the date,
/// the banner, the file name).
#[derive(Clone, Debug)]
pub struct PdfParams {
    /// `\pdfcompresslevel` (0–9).
    pub compress_level: i32,
    /// `\pdfdecimaldigits`.
    pub decimal_digits: i32,
    /// `\pdfmajorversion`.
    pub major_version: i32,
    /// `\pdfminorversion`.
    pub minor_version: i32,
    /// `\pdfhorigin`.
    pub h_origin: Scaled,
    /// `\pdfvorigin`.
    pub v_origin: Scaled,
    /// `\pdfpagewidth` (0: from the box).
    pub page_width: Scaled,
    /// `\pdfpageheight` (0: from the box).
    pub page_height: Scaled,
    /// `\hoffset`.
    pub h_offset: Scaled,
    /// `\voffset`.
    pub v_offset: Scaled,
    /// `\mag`.
    pub mag: i32,
    /// `start_time_str` (`texmfmp.c` `makepdftime`), e.g.
    /// `D:19700101000001Z`; see [`pdf_date`].
    pub date: String,
    /// `pdftex_banner`, the `/PTEX.Fullbanner`.
    pub banner: String,
    /// `output_file_name`, e.g. `doc.pdf`: the `/ID` is the MD5 of the
    /// date and this name (`print_ID`, `utils.c`).
    pub output_file_name: String,
}

impl Default for PdfParams {
    /// `IniTeX`'s values (pdfTeX §672).
    fn default() -> Self {
        PdfParams {
            compress_level: 9,
            decimal_digits: 3,
            major_version: 1,
            minor_version: 4,
            h_origin: (ONE_HUNDRED_INCH + 50) / 100,
            v_origin: (ONE_HUNDRED_INCH + 50) / 100,
            page_width: 0,
            page_height: 0,
            h_offset: 0,
            v_offset: 0,
            mag: 1000,
            date: pdf_date(0),
            banner: String::new(),
            output_file_name: String::new(),
        }
    }
}

/// `makepdftime(t, utc=true)` (`texmfmp.c`), as with `SOURCE_DATE_EPOCH`:
/// `D:YYYYMMDDHHMMSSZ` for `t` seconds since 1970.
#[must_use]
pub fn pdf_date(t: i64) -> String {
    let days = t.div_euclid(86400);
    let secs = t.rem_euclid(86400);
    // the civil date of a day number (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "D:{y:04}{m:02}{d:02}{:02}{:02}{:02}Z",
        secs / 3600,
        secs / 60 % 60,
        secs % 60
    )
}

/// The caller's fonts, by TeX's internal font number (`null_font` is 0).
pub trait FontSource {
    /// The metrics at the font's size (`font_size`, `char_width`, ...).
    fn metrics(&self, f: FontId) -> &ScaledFont;
    /// `font_name`: the TFM name, as the map file names it.
    fn name(&self, f: FontId) -> &[u8];
    /// The font's map entry (`pdftex.map`), if any.
    fn map_entry(&self, f: FontId) -> Option<MapEntry>;
    /// The bytes of the map entry's font file (`.pfb` or `.pfa`).
    fn font_file(&self, f: FontId) -> Option<Vec<u8>>;
    /// The bytes of the map entry's encoding file (`.enc`).
    fn encoding_file(&self, f: FontId) -> Option<Vec<u8>>;
}

/// A whatsit's handle and whether leaders are being output; the answer
/// is a `\special`'s bytes, or `None` (`phitex-dvi`'s shape). pdfTeX
/// reads a special's `PDF:`/`pdf:` prefix (pdfTeX §693 `literal`).
pub type Whatsits<'a> = dyn FnMut(u32, bool) -> Option<Vec<u8>> + 'a;

// object types (pdfTeX §695)
const OBJ_TYPE_OTHERS: usize = 0;
const OBJ_TYPE_PAGE: usize = 1;
const OBJ_TYPE_PAGES: usize = 2;
const OBJ_TYPE_FONT: usize = 3;
const HEAD_TAB_MAX: usize = 10;
/// `pages_tree_kids_max` (pdfTeX §695).
const PAGES_TREE_KIDS_MAX: i64 = 6;
/// `max_dimen`.
const MAX_DIMEN: Scaled = 0o7777777777;
/// `billion` (tex.web §625).
const BILLION: f64 = 1_000_000_000.0;

/// An `obj_entry` (pdfTeX §694–§695).
#[derive(Clone, Copy, Debug, Default)]
struct Obj {
    info: i64,
    link: usize,
    /// Negative until written (-2 fresh), then the byte offset.
    offset: i64,
}

/// What pdfTeX keeps per TeX font (pdfTeX §708, `font_used`), with the
/// caller's data for it.
struct FontState {
    name: Vec<u8>,
    metrics: ScaledFont,
    map: Option<MapEntry>,
    file: Option<Vec<u8>>,
    enc: Option<Vec<u8>>,
    used: bool,
    /// `pdf_font_size`.
    size: Scaled,
    /// `pdf_font_num`.
    num: i64,
    /// `pdf_char_used`.
    chars: [bool; 256],
}

impl FontState {
    /// `is_valid_char`.
    fn valid(&self, c: u16) -> bool {
        u8::try_from(c).is_ok_and(|c| self.metrics.char(c).is_some())
    }
    /// `char_width`, 0 for a character that doesn't exist.
    fn width(&self, c: u16) -> Scaled {
        u8::try_from(c)
            .ok()
            .and_then(|c| self.metrics.width(c))
            .unwrap_or(0)
    }
    /// `isscalable`.
    fn scalable(&self) -> bool {
        self.map.as_ref().is_some_and(|m| !m.is_pk())
    }
}

/// `fo_entry` (`writefont.c`).
struct Fo {
    objnum: usize,
    first_char: usize,
    last_char: usize,
    cw_objnum: usize,
    fd: usize,
    fe: Option<Vec<u8>>,
    fm: MapEntry,
}

/// `fd_entry` (`writefont.c`).
struct Fd {
    objnum: usize,
    fm: MapEntry,
    desc: FontDesc,
    file: Option<Vec<u8>>,
    ff_found: bool,
    ff_objnum: usize,
}

/// `fe_entry` (`writeenc.c`).
struct Fe {
    objnum: usize,
    names: GlyphNames,
    tx_tree: BTreeSet<usize>,
}

/// A box being output: what `pdf_hlist_out` and `pdf_vlist_out` keep of
/// `this_box` and their glue locals (pdfTeX §729, §738).
struct Frame {
    width: Scaled,
    height: Scaled,
    depth: Scaled,
    g_sign: GlueSign,
    g_order: Order,
    glue_set: f64,
    cur_g: Scaled,
    cur_glue: f64,
    /// `base_line` in an hlist, `top_edge` in a vlist.
    base: Scaled,
    left_edge: Scaled,
}

impl Frame {
    fn new(b: &BoxNode, base: Scaled, left_edge: Scaled) -> Frame {
        Frame {
            width: b.width,
            height: b.height,
            depth: b.depth,
            g_sign: b.glue.sign,
            g_order: b.glue.order,
            glue_set: b.glue.ratio,
            cur_g: 0,
            cur_glue: 0.0,
            base,
            left_edge,
        }
    }

    /// The glue's size as set (pdfTeX §735, §744).
    fn glue(&mut self, g: &GlueNode) -> Scaled {
        let spec = g.spec;
        let mut size = spec.width - self.cur_g;
        match self.g_sign {
            GlueSign::Stretching if spec.stretch_order == self.g_order => {
                self.cur_glue += f64::from(spec.stretch);
                self.cur_g = vet_round(self.glue_set * self.cur_glue);
            }
            GlueSign::Shrinking if spec.shrink_order == self.g_order => {
                self.cur_glue -= f64::from(spec.shrink);
                self.cur_g = vet_round(self.glue_set * self.cur_glue);
            }
            _ => {}
        }
        size += self.cur_g;
        size
    }
}

/// `vet_glue` then `round` (tex.web §625), in doubles as web2c computes.
#[allow(clippy::cast_possible_truncation)]
fn vet_round(glue_temp: f64) -> Scaled {
    let t = glue_temp.clamp(-BILLION, BILLION);
    if t >= 0.0 {
        (t + 0.5) as Scaled
    } else {
        (t - 0.5) as Scaled
    }
}

/// An `i64` from a count of objects or bytes.
fn int(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// The PDF writer: pdfTeX's state across pages.
#[allow(clippy::struct_excessive_bools)]
pub struct PdfWriter {
    p: PdfParams,
    out: Vec<u8>,
    /// The stream being compressed (`zip_write_state=zip_writing`).
    zip: Option<Vec<u8>>,
    stream_length_offset: usize,
    save_offset: usize,
    objs: Vec<Obj>,
    head: [usize; HEAD_TAB_MAX + 1],
    version_written: bool,
    initialized: bool,
    /// `fixed_decimal_digits`.
    dd: i32,
    min_bp_val: Scaled,
    total_pages: i64,
    last_pages: usize,
    // pdfTeX §691
    cur_h: Scaled,
    cur_v: Scaled,
    pdf_f: FontId,
    pdf_h: Scaled,
    pdf_v: Scaled,
    tj_start_h: Scaled,
    delta_h: Scaled,
    origin_h: Scaled,
    origin_v: Scaled,
    doing_string: bool,
    doing_text: bool,
    last_f: FontId,
    last_fs: Scaled,
    cur_page_width: Scaled,
    cur_page_height: Scaled,
    cur_s: i32,
    doing_leaders: bool,
    /// `pdf_font_list` of this page.
    font_list: Vec<FontId>,
    text_procset: bool,
    fonts: BTreeMap<FontId, FontState>,
    // the font files' state across fonts
    subset_tags: BTreeSet<[u8; 6]>,
    last_arg_other_subr3: i32,
}

impl PdfWriter {
    /// A writer; nothing is written before the first page.
    #[must_use]
    pub fn new(params: PdfParams) -> PdfWriter {
        PdfWriter {
            p: params,
            out: Vec::new(),
            zip: None,
            stream_length_offset: 0,
            save_offset: 0,
            objs: vec![Obj::default()],
            head: [0; HEAD_TAB_MAX + 1],
            version_written: false,
            initialized: false,
            dd: 3,
            min_bp_val: 0,
            total_pages: 0,
            last_pages: 0,
            cur_h: 0,
            cur_v: 0,
            pdf_f: 0,
            pdf_h: 0,
            pdf_v: 0,
            tj_start_h: 0,
            delta_h: 0,
            origin_h: 0,
            origin_v: 0,
            doing_string: false,
            doing_text: false,
            last_f: 0,
            last_fs: 0,
            cur_page_width: 0,
            cur_page_height: 0,
            cur_s: -1,
            doing_leaders: false,
            font_list: Vec::new(),
            text_procset: false,
            fonts: BTreeMap::new(),
            subset_tags: BTreeSet::new(),
            last_arg_other_subr3: 3,
        }
    }

    /// The number of pages shipped.
    #[must_use]
    pub fn total_pages(&self) -> i64 {
        self.total_pages
    }

    // ---- output (pdfTeX §680–§686) ----

    fn buf(&mut self) -> &mut Vec<u8> {
        match &mut self.zip {
            Some(z) => z,
            None => &mut self.out,
        }
    }

    fn pdf_out(&mut self, b: u8) {
        self.buf().push(b);
    }

    fn print(&mut self, s: &[u8]) {
        self.buf().extend_from_slice(s);
    }

    fn print_ln(&mut self, s: &[u8]) {
        self.print(s);
        self.pdf_out(b'\n');
    }

    fn print_int(&mut self, n: i64) {
        num::int(self.buf(), n);
    }

    fn print_int_ln(&mut self, n: i64) {
        self.print_int(n);
        self.pdf_out(b'\n');
    }

    fn print_real(&mut self, m: i32, d: i32) {
        num::real(self.buf(), m, d);
    }

    /// `pdf_print_bp`, returning `scaled_out`.
    fn print_bp(&mut self, s: Scaled) -> Scaled {
        let dd = self.dd;
        num::bp(self.buf(), s, dd)
    }

    /// `pdf_print_mag_bp`.
    fn print_mag_bp(&mut self, s: Scaled) {
        let s = if self.p.mag == 1000 {
            s
        } else {
            num::round_xn_over_d(s, self.p.mag, 1000)
        };
        self.print_bp(s);
    }

    /// `pdf_indirect_ln`.
    fn indirect_ln(&mut self, key: &[u8], o: usize) {
        self.pdf_out(b'/');
        self.print(key);
        self.pdf_out(b' ');
        self.print_int(int(o));
        self.print_ln(b" 0 R");
    }

    /// `remove_last_space`.
    fn remove_last_space(&mut self) {
        if self.buf().last() == Some(&b' ') {
            self.buf().pop();
        }
    }

    /// `pdf_offset`.
    fn offset(&self) -> usize {
        self.out.len()
    }

    /// `pdf_begin_stream`.
    fn begin_stream(&mut self) {
        self.print_ln(b"/Length           ");
        self.stream_length_offset = self.offset() - 11;
        if self.p.compress_level > 0 {
            self.print_ln(b"/Filter /FlateDecode");
            self.print_ln(b">>");
            self.print_ln(b"stream");
            self.zip = Some(Vec::new());
        } else {
            self.print_ln(b">>");
            self.print_ln(b"stream");
            self.save_offset = self.offset();
        }
    }

    /// `pdf_end_stream`, with `write_stream_length` (`utils.c`).
    fn end_stream(&mut self) {
        let length = if let Some(data) = self.zip.take() {
            let z = phitex_codecs::deflate::compress(&data, self.p.compress_level);
            self.out.extend_from_slice(&z);
            z.len()
        } else {
            self.offset() - self.save_offset
        };
        let digits = length.to_string();
        let at = self.stream_length_offset;
        self.out[at..at + digits.len()].copy_from_slice(digits.as_bytes());
        self.pdf_out(b'\n');
        self.print_ln(b"endstream");
        self.print_ln(b"endobj");
    }

    /// `check_pdfversion` (pdfTeX §683): the header.
    fn check_pdfversion(&mut self) {
        if self.version_written {
            return;
        }
        self.version_written = true;
        self.print(b"%PDF-");
        self.print_int(i64::from(self.p.major_version));
        self.pdf_out(b'.');
        self.print_int_ln(i64::from(self.p.minor_version));
        self.pdf_out(b'%');
        self.print(&[208, 212, 197, 216]);
        self.pdf_out(b'\n');
    }

    // ---- objects (pdfTeX §698) ----

    /// `pdf_create_obj(t, i)`.
    fn create_obj(&mut self, t: usize, i: i64) -> usize {
        self.objs.push(Obj {
            info: i,
            link: 0,
            offset: -2,
        });
        let n = self.objs.len() - 1;
        if t == OBJ_TYPE_PAGE {
            let mut p = self.head[t];
            if p == 0 || self.objs[p].info < i {
                self.objs[n].link = p;
                self.head[t] = n;
            } else {
                let mut q = p;
                while p != 0 {
                    if self.objs[p].info < i {
                        break;
                    }
                    q = p;
                    p = self.objs[p].link;
                }
                self.objs[q].link = n;
                self.objs[n].link = p;
            }
        } else if t != OBJ_TYPE_OTHERS {
            self.objs[n].link = self.head[t];
            self.head[t] = n;
        }
        n
    }

    /// `pdf_new_objnum`.
    fn new_objnum(&mut self) -> usize {
        self.create_obj(OBJ_TYPE_OTHERS, 0)
    }

    /// `pdf_begin_obj(i)` (no object streams).
    fn begin_obj(&mut self, i: usize) {
        self.check_pdfversion();
        self.objs[i].offset = int(self.offset());
        self.print_int(int(i));
        self.print_ln(b" 0 obj");
    }

    /// `pdf_begin_dict(i)`.
    fn begin_dict(&mut self, i: usize) {
        self.begin_obj(i);
        self.print_ln(b"<<");
    }

    /// `pdf_new_dict(t, i)`.
    fn new_dict(&mut self, t: usize, i: i64) -> usize {
        let n = self.create_obj(t, i);
        self.begin_dict(n);
        n
    }

    /// `pdf_end_obj`.
    fn end_obj(&mut self) {
        self.print_ln(b"endobj");
    }

    /// `pdf_end_dict`.
    fn end_dict(&mut self) {
        self.print_ln(b">>");
        self.print_ln(b"endobj");
    }

    /// `get_obj(obj_type_page, i, false)`: the page object for page `i`,
    /// created unless a reference made it.
    fn get_page_obj(&mut self, i: i64) -> usize {
        let mut k = self.head[OBJ_TYPE_PAGE];
        while k != 0 {
            if self.objs[k].info == i {
                return k;
            }
            k = self.objs[k].link;
        }
        self.create_obj(OBJ_TYPE_PAGE, i)
    }

    // ---- fonts on the page (pdfTeX §692–§693) ----

    fn font(&self, f: FontId) -> &FontState {
        &self.fonts[&f]
    }

    fn font_mut(&mut self, f: FontId) -> &mut FontState {
        self.fonts
            .get_mut(&f)
            .expect("a font is loaded before it is used")
    }

    /// The font's state, from the caller's data the first time.
    fn load_font(&mut self, f: FontId, src: &dyn FontSource) {
        if self.fonts.contains_key(&f) {
            return;
        }
        let map = src.map_entry(f);
        let file = map
            .as_ref()
            .filter(|m| m.is_fontfile())
            .and_then(|_| src.font_file(f));
        let enc = map
            .as_ref()
            .filter(|m| m.is_reencoded())
            .and_then(|_| src.encoding_file(f));
        self.fonts.insert(
            f,
            FontState {
                name: src.name(f).to_vec(),
                metrics: src.metrics(f).clone(),
                map,
                file,
                enc,
                used: false,
                size: 0,
                num: 0,
                chars: [false; 256],
            },
        );
    }

    /// `set_ff`.
    fn ff(&self, f: FontId) -> FontId {
        let n = self.font(f).num;
        if n < 0 {
            FontId::try_from(-n).unwrap_or(0)
        } else {
            f
        }
    }

    /// `pdf_use_font`.
    fn use_font(&mut self, f: FontId, num: i64) {
        let st = self.font_mut(f);
        st.size = divide_scaled(st.metrics.size, ONE_HUNDRED_BP, 6).1;
        st.used = true;
        st.num = num;
    }

    /// `pdf_init_font`.
    fn init_font(&mut self, f: FontId) {
        if self.font(f).scalable() {
            let mut i = self.head[OBJ_TYPE_FONT];
            while i != 0 {
                let k = FontId::try_from(self.objs[i].info).unwrap_or(0);
                let (fk, ff) = (self.font(k), self.font(f));
                if fk.scalable() && fk.map == ff.map && fk.name == ff.name {
                    let n = fk.num;
                    if n < 0 {
                        self.use_font(f, n);
                    } else {
                        self.use_font(f, -i64::from(k));
                    }
                    return;
                }
                i = self.objs[i].link;
            }
        }
        let n = self.create_obj(OBJ_TYPE_FONT, i64::from(f));
        self.use_font(f, int(n));
    }

    /// `pdf_set_font`.
    fn set_font(&mut self, f: FontId) {
        self.pdf_f = f;
        if !self.font(f).used {
            self.init_font(f);
        }
        let k = self.ff(f);
        if !self.font_list.iter().any(|&g| self.ff(g) == k) {
            self.font_list.push(f);
        }
        let size = self.font(f).metrics.size;
        if k == self.last_f && size == self.last_fs {
            return;
        }
        self.print(b"/F");
        self.print_int(i64::from(k));
        self.pdf_out(b' ');
        let q = divide_scaled(size, ONE_HUNDRED_BP, 6).0;
        self.print_real(q, 4);
        self.print(b" Tf");
        self.last_f = k;
        self.last_fs = size;
    }

    /// `pdf_set_origin`.
    fn set_origin(&mut self, h: Scaled, v: Scaled) {
        if (h - self.origin_h).abs() >= self.min_bp_val
            || (v - self.origin_v).abs() >= self.min_bp_val
        {
            self.print(b"1 0 0 1 ");
            let so = self.print_bp(h - self.origin_h);
            self.origin_h += so;
            self.pdf_out(b' ');
            let so = self.print_bp(self.origin_v - v);
            self.origin_v -= so;
            self.print_ln(b" cm");
        }
        self.pdf_h = self.origin_h;
        self.tj_start_h = self.pdf_h;
        self.pdf_v = self.origin_v;
    }

    /// `pdf_set_origin_temp`.
    fn set_origin_temp(&mut self, h: Scaled, v: Scaled) {
        if (h - self.origin_h).abs() >= self.min_bp_val
            || (v - self.origin_v).abs() >= self.min_bp_val
        {
            self.print(b"1 0 0 1 ");
            self.print_bp(h - self.origin_h);
            self.pdf_out(b' ');
            self.print_bp(self.origin_v - v);
            self.print_ln(b" cm");
        }
    }

    /// `pdf_end_string`.
    fn end_string(&mut self) {
        if self.doing_string {
            self.print(b")]TJ");
            self.doing_string = false;
        }
    }

    /// `pdf_end_string_nl`.
    fn end_string_nl(&mut self) {
        if self.doing_string {
            self.print_ln(b")]TJ");
            self.doing_string = false;
        }
    }

    /// `pdf_set_text_pos` (unexpanded fonts).
    fn set_text_pos(&mut self, v: i32, v_out: Scaled) {
        self.pdf_out(b' ');
        let so = self.print_bp(self.cur_h - self.tj_start_h);
        self.pdf_h = self.tj_start_h + so;
        self.pdf_out(b' ');
        self.print_real(v, self.dd);
        self.pdf_v -= v_out;
        self.print(b" Td");
        self.tj_start_h = self.pdf_h;
        self.delta_h = 0;
    }

    /// `pdf_begin_text`.
    fn begin_text(&mut self) {
        self.set_origin(0, self.cur_page_height);
        self.print_ln(b"BT");
        self.doing_text = true;
        self.pdf_f = 0;
        self.last_f = 0;
        self.last_fs = 0;
        self.doing_string = false;
    }

    /// `pdf_begin_string` (no faked interword spaces, no expanded fonts).
    fn begin_string(&mut self, f: FontId) {
        let mut must_set_text_pos = false;
        if !self.doing_text {
            self.begin_text();
            must_set_text_pos = true;
        }
        if self.pdf_f != f {
            self.end_string();
            self.set_font(f);
        }
        let (mut s, s_out) = divide_scaled(
            self.cur_h - (self.tj_start_h + self.delta_h),
            self.font(f).size,
            3,
        );
        let (v, v_out) = if (self.cur_v - self.pdf_v).abs() >= self.min_bp_val {
            divide_scaled(self.pdf_v - self.cur_v, ONE_HUNDRED_BP, self.dd + 2)
        } else {
            (0, 0)
        };
        if !must_set_text_pos {
            must_set_text_pos = v != 0 || s.abs() >= 0o100000;
        }
        if must_set_text_pos {
            self.end_string();
            self.set_font(f);
            self.set_text_pos(v, v_out);
            s = 0;
        }
        if !self.doing_string {
            self.print(b" [");
            if s == 0 {
                self.pdf_out(b'(');
            }
        }
        if s != 0 {
            if self.doing_string {
                self.pdf_out(b')');
            }
            self.print_int(-i64::from(s));
            self.pdf_out(b'(');
            self.delta_h += s_out;
        }
        self.doing_string = true;
    }

    /// `pdf_end_text`.
    fn end_text(&mut self) {
        if self.doing_text {
            self.end_string_nl();
            self.print_ln(b"ET");
            self.doing_text = false;
        }
    }

    /// `pdf_print_char`.
    fn print_char(&mut self, f: FontId, c: u8) {
        self.font_mut(f).chars[usize::from(c)] = true;
        if c <= 32 || c == 92 || c == 40 || c == 41 || c > 127 {
            self.pdf_out(b'\\');
            let o = format!("{c:03o}");
            self.print(o.as_bytes());
        } else {
            self.pdf_out(c);
        }
    }

    /// `adv_char_width` (unexpanded fonts).
    fn adv_char_width(&mut self, f: FontId, c: u16) {
        let st = self.font(f);
        let (_, so) = divide_scaled(st.width(c), st.size, 4);
        self.delta_h += so;
    }

    /// `pdf_set_rule` (pdfTeX §693).
    fn set_rule(&mut self, x: Scaled, y: Scaled, w: Scaled, h: Scaled) {
        self.end_text();
        self.print_ln(b"q");
        if h <= ONE_BP {
            self.set_origin_temp(x, y - (h + 1) / 2);
            self.print(b"[]0 d 0 J ");
            self.print_bp(h);
            self.print(b" w 0 0 m ");
            self.print_bp(w);
            self.print_ln(b" 0 l S");
        } else if w <= ONE_BP {
            self.set_origin_temp(x + (w + 1) / 2, y);
            self.print(b"[]0 d 0 J ");
            self.print_bp(w);
            self.print(b" w 0 0 m 0 ");
            self.print_bp(h);
            self.print_ln(b" l S");
        } else {
            self.set_origin_temp(x, y);
            self.print(b"0 0 ");
            self.print_bp(w);
            self.pdf_out(b' ');
            self.print_bp(h);
            self.print_ln(b" re f");
        }
        self.print_ln(b"Q");
    }

    /// `literal(s, scan_special, true)` (pdfTeX §693) for a `\special`.
    fn special(&mut self, s: &[u8]) {
        let Some(rest) = s.strip_prefix(b"PDF:").or_else(|| s.strip_prefix(b"pdf:")) else {
            // "Non-PDF special ignored!" goes to the log
            return;
        };
        let body = if let Some(b) = rest.strip_prefix(b"direct:") {
            self.end_string_nl();
            b
        } else if let Some(b) = rest.strip_prefix(b"page:") {
            self.end_text();
            b
        } else {
            self.end_text();
            self.set_origin(self.cur_h, self.cur_v);
            rest
        };
        self.print(body);
        self.pdf_out(b'\n');
    }

    // ---- lists (pdfTeX §729–§746) ----

    fn box_out(&mut self, b: &BoxNode, src: &dyn FontSource, wh: &mut Whatsits<'_>) {
        match b.kind {
            BoxKind::H => self.hlist_out(b, src, wh),
            BoxKind::V => self.vlist_out(b, src, wh),
        }
    }

    fn whatsit(&mut self, h: u32, wh: &mut Whatsits<'_>) {
        if let Some(s) = wh(h, self.doing_leaders) {
            self.special(&s);
        }
    }

    /// A character in an hlist (pdfTeX §731, `output_one_char` §726).
    fn char_out(&mut self, f: FontId, c: u16, src: &dyn FontSource) {
        self.load_font(f, src);
        if self.font(f).valid(c) {
            let b = u8::try_from(c).unwrap_or(0);
            self.begin_string(f);
            self.print_char(f, b);
            self.adv_char_width(f, c);
        }
        self.cur_h += self.font(f).width(c);
    }

    /// `pdf_hlist_out` (pdfTeX §729).
    fn hlist_out(&mut self, b: &BoxNode, src: &dyn FontSource, wh: &mut Whatsits<'_>) {
        self.cur_s += 1;
        let mut fr = Frame::new(b, self.cur_v, self.cur_h);
        self.hlist_nodes(&b.list, &mut fr, src, wh);
        self.cur_s -= 1;
    }

    /// The nodes of an hlist (pdfTeX §731–§737); a discretionary's
    /// replacement text is part of the list.
    fn hlist_nodes(
        &mut self,
        list: &[Node],
        fr: &mut Frame,
        src: &dyn FontSource,
        wh: &mut Whatsits<'_>,
    ) {
        for p in list {
            let rule = match p {
                Node::Char { font, ch } => {
                    self.char_out(*font, *ch, src);
                    None
                }
                Node::Lig(l) => {
                    self.char_out(l.font, l.ch, src);
                    None
                }
                Node::Box(bx) => {
                    // §733
                    if bx.list.is_empty() {
                        self.cur_h += bx.width;
                    } else {
                        self.cur_v = fr.base + bx.shift;
                        let edge = self.cur_h + bx.width;
                        self.box_out(bx, src, wh);
                        self.cur_h = edge;
                        self.cur_v = fr.base;
                    }
                    None
                }
                Node::Rule(r) => Some((r.height, r.depth, r.width)),
                Node::Whatsit(h) => {
                    self.whatsit(*h, wh);
                    None
                }
                Node::Glue(g) => self.hglue(g, fr, src, wh),
                Node::Kern(w, _) | Node::Math(_, w) => {
                    self.cur_h += w;
                    None
                }
                Node::Disc(d) => {
                    self.hlist_nodes(&d.replace, fr, src, wh);
                    None
                }
                Node::Unset(_)
                | Node::Ins(_)
                | Node::Mark(_)
                | Node::Adjust(_)
                | Node::Language { .. }
                | Node::Penalty(_) => None,
            };
            if let Some((mut rule_ht, mut rule_dp, rule_wd)) = rule {
                // §734
                if rule_ht == RUNNING {
                    rule_ht = fr.height;
                }
                if rule_dp == RUNNING {
                    rule_dp = fr.depth;
                }
                rule_ht += rule_dp;
                if rule_ht > 0 && rule_wd > 0 {
                    self.cur_v = fr.base + rule_dp;
                    self.set_rule(self.cur_h, self.cur_v, rule_wd, rule_ht);
                    self.cur_v = fr.base;
                }
                self.cur_h += rule_wd;
            }
        }
    }

    /// Glue in an hlist (pdfTeX §735–§737): the rule to output, or `None`
    /// once `cur_h` has moved past it.
    fn hglue(
        &mut self,
        g: &GlueNode,
        fr: &mut Frame,
        src: &dyn FontSource,
        wh: &mut Whatsits<'_>,
    ) -> Option<(Scaled, Scaled, Scaled)> {
        let mut rule_wd = fr.glue(g);
        if matches!(
            g.kind,
            GlueKind::ALeaders | GlueKind::CLeaders | GlueKind::XLeaders
        ) {
            match g.leader.as_deref() {
                Some(Node::Rule(r)) => return Some((r.height, r.depth, rule_wd)),
                Some(Node::Box(lb)) => {
                    let leader_wd = lb.width;
                    if leader_wd > 0 && rule_wd > 0 {
                        rule_wd += 10;
                        let edge = self.cur_h + rule_wd;
                        let mut lx = 0;
                        if g.kind == GlueKind::ALeaders {
                            let save_h = self.cur_h;
                            self.cur_h = fr.left_edge
                                + leader_wd * ((self.cur_h - fr.left_edge) / leader_wd);
                            if self.cur_h < save_h {
                                self.cur_h += leader_wd;
                            }
                        } else {
                            let lq = rule_wd / leader_wd;
                            let lr = rule_wd % leader_wd;
                            if g.kind == GlueKind::CLeaders {
                                self.cur_h += lr / 2;
                            } else {
                                lx = lr / (lq + 1);
                                self.cur_h += (lr - (lq - 1) * lx) / 2;
                            }
                        }
                        while self.cur_h + leader_wd <= edge {
                            // §737
                            self.cur_v = fr.base + lb.shift;
                            let save_h = self.cur_h;
                            let outer = self.doing_leaders;
                            self.doing_leaders = true;
                            self.box_out(lb, src, wh);
                            self.doing_leaders = outer;
                            self.cur_v = fr.base;
                            self.cur_h = save_h + leader_wd + lx;
                        }
                        self.cur_h = edge - 10;
                        return None;
                    }
                }
                _ => {}
            }
        }
        self.cur_h += rule_wd;
        None
    }

    /// `pdf_vlist_out` (pdfTeX §738–§746).
    fn vlist_out(&mut self, b: &BoxNode, src: &dyn FontSource, wh: &mut Whatsits<'_>) {
        self.cur_s += 1;
        let left_edge = self.cur_h;
        self.cur_v -= b.height;
        let mut fr = Frame::new(b, self.cur_v, left_edge);
        for p in &b.list {
            let rule = match p {
                Node::Char { .. } | Node::Lig(_) => panic!("pdfvlistout"),
                Node::Box(bx) => {
                    // §742
                    if bx.list.is_empty() {
                        self.cur_v += bx.height + bx.depth;
                    } else {
                        self.cur_v += bx.height;
                        let save_v = self.cur_v;
                        self.cur_h = left_edge + bx.shift;
                        self.box_out(bx, src, wh);
                        self.cur_v = save_v + bx.depth;
                        self.cur_h = left_edge;
                    }
                    None
                }
                Node::Rule(r) => Some((r.height, r.depth, r.width)),
                Node::Whatsit(h) => {
                    self.whatsit(*h, wh);
                    None
                }
                Node::Glue(g) => self.vglue(g, &mut fr, src, wh),
                Node::Kern(w, _) => {
                    self.cur_v += w;
                    None
                }
                _ => None,
            };
            if let Some((rule_ht, rule_dp, mut rule_wd)) = rule {
                // §743
                if rule_wd == RUNNING {
                    rule_wd = fr.width;
                }
                let rule_ht = rule_ht + rule_dp;
                self.cur_v += rule_ht;
                if rule_ht > 0 && rule_wd > 0 {
                    self.set_rule(self.cur_h, self.cur_v, rule_wd, rule_ht);
                    self.cur_h = left_edge;
                }
            }
        }
        self.cur_s -= 1;
    }

    /// Glue in a vlist (pdfTeX §744–§746).
    fn vglue(
        &mut self,
        g: &GlueNode,
        fr: &mut Frame,
        src: &dyn FontSource,
        wh: &mut Whatsits<'_>,
    ) -> Option<(Scaled, Scaled, Scaled)> {
        let mut rule_ht = fr.glue(g);
        if matches!(
            g.kind,
            GlueKind::ALeaders | GlueKind::CLeaders | GlueKind::XLeaders
        ) {
            match g.leader.as_deref() {
                Some(Node::Rule(r)) => return Some((rule_ht, 0, r.width)),
                Some(Node::Box(lb)) => {
                    let leader_ht = lb.height + lb.depth;
                    if leader_ht > 0 && rule_ht > 0 {
                        rule_ht += 10;
                        let edge = self.cur_v + rule_ht;
                        let mut lx = 0;
                        if g.kind == GlueKind::ALeaders {
                            let save_v = self.cur_v;
                            self.cur_v = fr.base + leader_ht * ((self.cur_v - fr.base) / leader_ht);
                            if self.cur_v < save_v {
                                self.cur_v += leader_ht;
                            }
                        } else {
                            let lq = rule_ht / leader_ht;
                            let lr = rule_ht % leader_ht;
                            if g.kind == GlueKind::CLeaders {
                                self.cur_v += lr / 2;
                            } else {
                                lx = lr / (lq + 1);
                                self.cur_v += (lr - (lq - 1) * lx) / 2;
                            }
                        }
                        while self.cur_v + leader_ht <= edge {
                            // §746
                            self.cur_h = fr.left_edge + lb.shift;
                            self.cur_v += lb.height;
                            let save_v = self.cur_v;
                            let outer = self.doing_leaders;
                            self.doing_leaders = true;
                            self.box_out(lb, src, wh);
                            self.doing_leaders = outer;
                            self.cur_h = fr.left_edge;
                            self.cur_v = save_v - lb.height + leader_ht + lx;
                        }
                        self.cur_v = edge - 10;
                        return None;
                    }
                }
                _ => {}
            }
        }
        self.cur_v += rule_ht;
        None
    }

    // ---- shipping out (pdfTeX §750–§771) ----

    /// `pdf_ship_out(p, true)`: one page. `fonts` answers for each font
    /// the first time it is used; `whatsits` answers specials.
    ///
    /// # Errors
    /// [`PdfError::HugePage`] for a page TeX refuses; nothing is written.
    pub fn ship_out(
        &mut self,
        page: &BoxNode,
        fonts: &dyn FontSource,
        whatsits: &mut Whatsits<'_>,
    ) -> Result<(), PdfError> {
        if !self.initialized {
            // §792
            self.check_pdfversion();
            self.dd = self.p.decimal_digits.clamp(0, 4);
            let ten = TEN_POW[usize::try_from(self.dd + 2).unwrap_or(0)];
            self.min_bp_val = divide_scaled(ONE_HUNDRED_BP, i32::try_from(ten).unwrap_or(1), 0).0;
            self.initialized = true;
        }
        // §751: tex.web §641's test
        let too_big = |a: Scaled, b: Scaled| a.checked_add(b).is_none_or(|s| s > MAX_DIMEN);
        if page.height > MAX_DIMEN
            || page.depth > MAX_DIMEN
            || page
                .height
                .checked_add(page.depth)
                .is_none_or(|hd| too_big(hd, self.p.v_offset))
            || too_big(page.width, self.p.h_offset)
        {
            return Err(PdfError::HugePage);
        }
        // §752
        let last_resources = self.new_objnum();
        self.font_list.clear();
        self.text_procset = false;
        // §755
        let cur_h_offset = self.p.h_origin + self.p.h_offset;
        let cur_v_offset = self.p.v_origin + self.p.v_offset;
        self.cur_page_width = if self.p.page_width != 0 {
            self.p.page_width
        } else {
            page.width + 2 * cur_h_offset
        };
        self.cur_page_height = if self.p.page_height != 0 {
            self.p.page_height
        } else {
            page.height + page.depth + 2 * cur_v_offset
        };
        let last_page = self.get_page_obj(self.total_pages + 1);
        let last_stream = self.new_dict(OBJ_TYPE_OTHERS, 0);
        self.cur_h = cur_h_offset;
        self.cur_v = page.height + cur_v_offset;
        self.origin_h = 0;
        self.origin_v = self.cur_page_height;
        // §757–§758
        self.begin_stream();
        if self.p.mag != 1000 {
            self.print_real(self.p.mag, 3);
            self.print(b" 0 0 ");
            self.print_real(self.p.mag, 3);
            self.print_ln(b" 0 0 cm");
        }
        self.box_out(page, fonts, whatsits);
        self.total_pages += 1;
        self.cur_s = -1;
        // §759–§760
        self.end_text();
        self.end_stream();
        // §769
        self.begin_dict(last_page);
        self.print_ln(b"/Type /Page");
        self.indirect_ln(b"Contents", last_stream);
        self.indirect_ln(b"Resources", last_resources);
        self.print(b"/MediaBox [0 0 ");
        self.print_mag_bp(self.cur_page_width);
        self.pdf_out(b' ');
        self.print_mag_bp(self.cur_page_height);
        self.print_ln(b"]");
        // §770
        if self.total_pages % PAGES_TREE_KIDS_MAX == 1 {
            self.last_pages = self.create_obj(OBJ_TYPE_PAGES, PAGES_TREE_KIDS_MAX);
        }
        self.indirect_ln(b"Parent", self.last_pages);
        self.end_dict();
        // §762
        self.begin_dict(last_resources);
        if !self.font_list.is_empty() {
            // §766
            self.print(b"/Font << ");
            for f in self.font_list.clone() {
                let k = self.ff(f);
                self.print(b"/F");
                self.print_int(i64::from(k));
                self.pdf_out(b' ');
                self.print_int(self.font(k).num);
                self.print(b" 0 R ");
            }
            self.print_ln(b">>");
            self.text_procset = true;
        }
        // §768
        if self.p.major_version < 2 {
            self.print(b"/ProcSet [ /PDF");
            if self.text_procset {
                self.print(b" /Text");
            }
            self.print_ln(b" ]");
        }
        self.end_dict();
        Ok(())
    }

    // ---- fonts (writefont.c) ----

    /// `get_char_range`: the first and last characters used.
    fn char_range(&self, f: FontId) -> (usize, usize) {
        let st = self.font(f);
        let first = (0..256).find(|&i| st.chars[i]);
        let last = (0..256).rev().find(|&i| st.chars[i]);
        match (first, last) {
            (Some(a), Some(b)) => (a, b),
            _ => (1, 0),
        }
    }

    /// `preset_fontmetrics`: the descriptor's values from the TFM.
    fn preset_fontmetrics(&self, f: FontId) -> [(i32, bool); 11] {
        let st = self.font(f);
        let m = &st.metrics;
        let size = st.size;
        let d = |x: Scaled| divide_scaled(x, size, 3).0;
        let h = |c: u8| m.char(c).map_or(0, |x| x.height);
        let mut dims = [(0, false); 11];
        #[allow(clippy::cast_possible_truncation)]
        let angle =
            (-(f64::from(m.slant()) / 65536.0).atan() * (180.0 / std::f64::consts::PI)) as i32;
        dims[dim::ITALIC_ANGLE].0 = d(angle);
        dims[dim::ASCENT].0 = d(h(b'h'));
        dims[dim::CAPHEIGHT].0 = d(h(b'H'));
        let i = -d(m.char(b'y').map_or(0, |x| x.depth));
        dims[dim::DESCENT].0 = if i < 0 { i } else { 0 };
        dims[dim::STEMV].0 = d(m.width(b'.').unwrap_or(0) / 3);
        dims[dim::XHEIGHT].0 = d(m.x_height());
        dims[dim::FONTBBOX1].0 = 0;
        dims[dim::FONTBBOX1 + 1].0 = dims[dim::DESCENT].0;
        dims[dim::FONTBBOX1 + 2].0 = d(m.quad());
        dims[dim::FONTBBOX1 + 3].0 = dims[dim::CAPHEIGHT].0.max(dims[dim::ASCENT].0);
        for x in dims.iter_mut().take(dim::INT_KEYS_NUM) {
            x.1 = true;
        }
        dims
    }

    /// `write_charwidth_array`, after `create_charwidth_array`.
    fn write_widths(&mut self, f: FontId, first: usize, last: usize) -> usize {
        let n = self.new_objnum();
        self.begin_obj(n);
        self.print(b"[");
        for i in first..=last {
            let st = self.font(f);
            let w = divide_scaled(st.width(u16::try_from(i).unwrap_or(0)), st.size, 4).0;
            self.print_int(i64::from(w / 10));
            if w % 10 != 0 {
                self.pdf_out(b'.');
                self.print_int(i64::from(w % 10));
            }
            if i != last {
                self.pdf_out(b' ');
            }
        }
        self.print(b"]\n");
        self.end_obj();
        n
    }

    /// `write_fontname`.
    fn write_fontname(&mut self, desc: &FontDesc, key: &[u8]) {
        self.pdf_out(b'/');
        self.print(key);
        self.print(b" /");
        if let Some(t) = desc.subset_tag {
            self.print(&t);
            self.pdf_out(b'+');
        }
        self.print(&desc.fontname);
        self.pdf_out(b'\n');
    }

    /// `write_fontdescriptor`, with `write_fontfile`.
    fn write_fontdescriptor(&mut self, fd: &mut Fd) -> Result<(), PdfError> {
        if fd.fm.is_fontfile() {
            // write_fontfile
            if !fd.fm.is_type1() {
                return Err(PdfError::Unsupported("TrueType and OpenType fonts".into()));
            }
            let Some(file) = fd.file.take() else {
                return Err(PdfError::Font(format!(
                    "cannot open Type 1 font file for reading: {}",
                    String::from_utf8_lossy(fd.fm.ff_name.as_deref().unwrap_or_default())
                )));
            };
            let e = t1::write_t1(
                &file,
                &fd.fm,
                &mut fd.desc,
                &mut self.subset_tags,
                &mut self.last_arg_other_subr3,
            )?;
            fd.ff_found = true;
            fd.ff_objnum = self.new_objnum();
            self.begin_dict(fd.ff_objnum);
            self.print(
                format!(
                    "/Length1 {}\n/Length2 {}\n/Length3 {}\n",
                    e.length1, e.length2, e.length3
                )
                .as_bytes(),
            );
            self.begin_stream();
            self.print(&e.bytes);
            self.end_stream();
        }
        if fd.objnum == 0 {
            fd.objnum = self.new_objnum();
        }
        self.begin_dict(fd.objnum);
        self.print(b"/Type /FontDescriptor\n");
        self.write_fontname(&fd.desc, b"FontName");
        let flags = match fd.fm.fd_flags {
            Some(fl) => fl,
            None if fd.ff_found => 4,
            None if fd.fm.is_std_t1font() => {
                return Err(PdfError::Unsupported("the 14 standard fonts".into()));
            }
            None => 0x22,
        };
        self.print(format!("/Flags {flags}\n").as_bytes());
        // write_fontmetrics, fix_fontmetrics
        let bb = dim::FONTBBOX1;
        let dims = &mut fd.desc.font_dim;
        if dims[bb].1 && dims[bb + 1].1 && dims[bb + 2].1 && dims[bb + 3].1 {
            if !dims[dim::ASCENT].1 {
                dims[dim::ASCENT] = (dims[bb + 3].0, true);
            }
            if !dims[dim::DESCENT].1 {
                dims[dim::DESCENT] = (dims[bb + 1].0, true);
            }
            if !dims[dim::CAPHEIGHT].1 {
                dims[dim::CAPHEIGHT] = (dims[bb + 3].0, true);
            }
        }
        let dims = fd.desc.font_dim;
        if dims[bb].1 && dims[bb + 1].1 && dims[bb + 2].1 && dims[bb + 3].1 {
            self.print(
                format!(
                    "/FontBBox [{} {} {} {}]\n",
                    dims[bb].0,
                    dims[bb + 1].0,
                    dims[bb + 2].0,
                    dims[bb + 3].0
                )
                .as_bytes(),
            );
        }
        for (i, (key, _)) in t1::FONT_KEY.iter().enumerate().take(dim::GEN_KEY_NUM) {
            if dims[i].1 {
                self.print(format!("/{key} {}\n", dims[i].0).as_bytes());
            }
        }
        if fd.ff_found {
            if fd.fm.is_subsetted() && fd.fm.is_type1() {
                self.print(b"/CharSet (");
                for g in &fd.desc.gl_tree {
                    self.pdf_out(b'/');
                    self.print(g);
                }
                self.print(b")\n");
            }
            self.print(format!("/FontFile {} 0 R\n", fd.ff_objnum).as_bytes());
        }
        self.end_dict();
        Ok(())
    }

    /// A font descriptor made from TeX font `f`'s data (`create_fontdescriptor`).
    fn new_fd(&self, fm: &MapEntry, f: FontId) -> Fd {
        Fd {
            objnum: 0,
            fm: fm.clone(),
            desc: FontDesc {
                fontname: fm.ps_name.clone().unwrap_or_else(|| fm.tfm_name.clone()),
                subset_tag: None,
                font_dim: self.preset_fontmetrics(f),
                gl_tree: BTreeSet::new(),
                tx_tree: None,
                builtin: None,
            },
            file: self.font(f).file.clone(),
            ff_found: false,
            ff_objnum: 0,
        }
    }

    /// `create_fontdictionary`.
    fn create_fontdictionary(
        &mut self,
        fm: MapEntry,
        objnum: usize,
        f: FontId,
        t: &mut FontTables,
    ) -> Result<(), PdfError> {
        let (first, last) = self.char_range(f);
        let marked: Vec<usize> = (first..=last).filter(|&i| self.font(f).chars[i]).collect();
        let mut fe_key = None;
        if let Some(encname) = fm.encname.clone() {
            if !t.fe_tree.contains_key(&encname) {
                let Some(bytes) = self.font(f).enc.clone() else {
                    return Err(PdfError::Font(format!(
                        "cannot open encoding file for reading: {}",
                        String::from_utf8_lossy(&encname)
                    )));
                };
                let names = enc::load_enc(&bytes)?;
                t.fe_tree.insert(
                    encname.clone(),
                    Fe {
                        objnum: 0,
                        names,
                        tx_tree: BTreeSet::new(),
                    },
                );
            }
            if fm.is_type1() || fm.is_opentype() {
                let need = t.fe_tree[&encname].objnum == 0;
                let n = if need { self.new_objnum() } else { 0 };
                let fe = t.fe_tree.get_mut(&encname).expect("registered above");
                if need {
                    fe.objnum = n;
                }
                fe.tx_tree.extend(marked.iter().copied());
            }
            fe_key = Some(encname);
        }
        let fd_index;
        let cw_objnum;
        if fm.is_fontfile() {
            if !fm.is_type1() {
                return Err(PdfError::Unsupported("TrueType and OpenType fonts".into()));
            }
            let key = (fm.ff_name.clone().unwrap_or_default(), fm.slant, fm.extend);
            fd_index = if let Some(&i) = t.fd_tree.get(&key) {
                i
            } else {
                t.fds.push(self.new_fd(&fm, f));
                t.fd_tree.insert(key, t.fds.len() - 1);
                t.fds.len() - 1
            };
            cw_objnum = self.write_widths(f, first, last);
            let fd = &mut t.fds[fd_index];
            if let Some(k) = &fe_key {
                if fm.is_subsetted() {
                    let g = &t.fe_tree[k].names;
                    for &i in &marked {
                        if let Some(n) = &g[i] {
                            fd.desc.gl_tree.insert(n.clone());
                        }
                    }
                }
            } else {
                fd.desc
                    .tx_tree
                    .get_or_insert_with(BTreeSet::new)
                    .extend(marked.iter().copied());
            }
        } else {
            // a builtin font: its widths and descriptor now
            cw_objnum = self.write_widths(f, first, last);
            let mut fd = self.new_fd(&fm, f);
            self.write_fontdescriptor(&mut fd)?;
            t.fds.push(fd);
            fd_index = t.fds.len() - 1;
        }
        t.fo_tree.insert(
            fm.tfm_name.clone(),
            Fo {
                objnum,
                first_char: first,
                last_char: last,
                cw_objnum,
                fd: fd_index,
                fe: fe_key,
                fm,
            },
        );
        Ok(())
    }

    /// `write_fontdictionary`.
    fn write_fontdictionary(&mut self, fo: &Fo, t: &FontTables) {
        self.begin_dict(fo.objnum);
        self.print(b"/Type /Font\n");
        self.print(b"/Subtype /Type1\n");
        let fd = &t.fds[fo.fd];
        self.write_fontname(&fd.desc, b"BaseFont");
        self.print(format!("/FontDescriptor {} 0 R\n", fd.objnum).as_bytes());
        self.print(
            format!(
                "/FirstChar {}\n/LastChar {}\n/Widths {} 0 R\n",
                fo.first_char, fo.last_char, fo.cw_objnum
            )
            .as_bytes(),
        );
        if (fo.fm.is_type1() || fo.fm.is_opentype())
            && let Some(fe) = fo.fe.as_ref().and_then(|k| t.fe_tree.get(k))
            && fe.objnum != 0
        {
            self.print(format!("/Encoding {} 0 R\n", fe.objnum).as_bytes());
        }
        self.end_dict();
    }

    /// `write_enc` (`writeenc.c`).
    fn write_enc(&mut self, fe: &Fe) {
        self.begin_dict(fe.objnum);
        self.print(b"/Type /Encoding\n");
        self.print(b"/Differences [");
        let mut i_old: i64 = -2;
        for &c in &fe.tx_tree {
            let name = fe.names[c].as_deref().unwrap_or(b".notdef").to_vec();
            let c = int(c);
            if c == i_old + 1 {
                self.pdf_out(b'/');
                self.print(&name);
            } else {
                if i_old != -2 {
                    self.pdf_out(b' ');
                }
                self.print_int(c);
                self.pdf_out(b'/');
                self.print(&name);
            }
            i_old = c;
        }
        self.print(b"]\n");
        self.end_dict();
    }

    /// `@<Output fonts definition@>` (pdfTeX §801) and `write_fontstuff`.
    fn output_fonts(&mut self) -> Result<(), PdfError> {
        let ids: Vec<FontId> = self.fonts.keys().copied().collect();
        for k in ids {
            let st = self.font(k);
            if st.used && st.map.is_some() && st.num < 0 {
                let i = FontId::try_from(-st.num).unwrap_or(0);
                let chars = st.chars;
                for (b, c) in self.font_mut(i).chars.iter_mut().zip(chars) {
                    *b |= c;
                }
            }
        }
        let mut t = FontTables::default();
        let mut k = self.head[OBJ_TYPE_FONT];
        while k != 0 {
            let f = FontId::try_from(self.objs[k].info).unwrap_or(0);
            // do_pdf_font
            let (first, last) = self.char_range(f);
            if first <= last {
                match self.font(f).map.clone() {
                    Some(fm) if !fm.is_pk() => self.create_fontdictionary(fm, k, f, &mut t)?,
                    _ => {
                        return Err(PdfError::Unsupported(format!(
                            "PK (Type 3) font {}",
                            String::from_utf8_lossy(&self.font(f).name)
                        )));
                    }
                }
            }
            k = self.objs[k].link;
        }
        // write_fontdescriptors
        let order: Vec<usize> = t.fd_tree.values().copied().collect();
        for i in order {
            let mut fd = t.fds.swap_remove(i);
            let r = self.write_fontdescriptor(&mut fd);
            t.fds.push(fd);
            let last = t.fds.len() - 1;
            t.fds.swap(i, last);
            r?;
        }
        // write_fontencodings
        for fe in t.fe_tree.values() {
            if fe.objnum != 0 {
                self.write_enc(fe);
            }
        }
        // write_fontdictionaries
        for fo in t.fo_tree.values() {
            self.write_fontdictionary(fo, &t);
        }
        Ok(())
    }

    // ---- the end (pdfTeX §794–§815) ----

    /// The rest of the file: fonts, the pages tree, the catalog, the
    /// `/Info` dictionary, the cross-reference table and the trailer; no
    /// file at all without pages.
    ///
    /// # Errors
    /// A font that cannot be embedded.
    pub fn finish(mut self) -> Result<Vec<u8>, PdfError> {
        if self.total_pages == 0 {
            return Ok(Vec::new());
        }
        if self.total_pages % PAGES_TREE_KIDS_MAX != 0 {
            self.objs[self.last_pages].info = self.total_pages % PAGES_TREE_KIDS_MAX;
        }
        // §800 (§799: every page object here was shipped)
        let mut k = self.head[OBJ_TYPE_PAGE];
        let mut l = 0;
        while k != 0 {
            let i = self.objs[k].link;
            self.objs[k].link = l;
            l = k;
            k = i;
        }
        self.head[OBJ_TYPE_PAGE] = l;
        let mut k = self.head[OBJ_TYPE_PAGES];
        let pages_tail = k;
        let mut l = 0;
        while k != 0 {
            let i = self.objs[k].link;
            self.objs[k].link = l;
            l = k;
            k = i;
        }
        self.head[OBJ_TYPE_PAGES] = l;
        self.output_fonts()?;
        self.output_pages_tree(pages_tail);
        // §806
        let root = self.new_dict(OBJ_TYPE_OTHERS, 0);
        self.print_ln(b"/Type /Catalog");
        self.indirect_ln(b"Pages", self.last_pages);
        self.end_dict();
        // §807–§810
        self.new_dict(OBJ_TYPE_OTHERS, 0);
        self.print_ln(b"/Producer (pdfTeX-1.40.29)");
        self.print_ln(b"/Creator (TeX)");
        let date = self.p.date.clone();
        self.print(format!("/CreationDate ({date})\n").as_bytes());
        self.print(format!("/ModDate ({date})\n").as_bytes());
        self.print_ln(b"/Trapped /False");
        let banner = self.p.banner.clone();
        self.print(b"/PTEX.Fullbanner ");
        self.print_str(banner.as_bytes());
        self.pdf_out(b'\n');
        self.end_dict();
        // §812–§813
        let obj_ptr = self.objs.len() - 1;
        let mut l = 0;
        self.objs[0].offset = -2;
        for k in 1..=obj_ptr {
            if self.objs[k].offset < 0 {
                self.objs[l].link = k;
                l = k;
            }
        }
        self.objs[l].link = 0;
        self.save_offset = self.offset();
        self.print_ln(b"xref");
        self.print(b"0 ");
        self.print_int_ln(int(obj_ptr) + 1);
        self.print_fw_int(int(self.objs[0].link), 10);
        self.print_ln(b" 65535 f ");
        for k in 1..=obj_ptr {
            if self.objs[k].offset < 0 {
                self.print_fw_int(int(self.objs[k].link), 10);
                self.print_ln(b" 00000 f ");
            } else {
                self.print_fw_int(self.objs[k].offset, 10);
                self.print_ln(b" 00000 n ");
            }
        }
        // §815
        self.print_ln(b"trailer");
        self.print(b"<< ");
        self.print(b"/Size ");
        self.print_int_ln(int(obj_ptr) + 1);
        self.indirect_ln(b"Root", root);
        self.indirect_ln(b"Info", obj_ptr);
        // print_ID (utils.c)
        let mut m = self.p.date.clone().into_bytes();
        m.extend_from_slice(self.p.output_file_name.as_bytes());
        let id = phitex_codecs::md5::md5(&m)
            .iter()
            .fold(String::new(), |mut s, b| {
                use std::fmt::Write;
                let _ = write!(s, "{b:02X}");
                s
            });
        self.print(format!("/ID [<{id}> <{id}>]").as_bytes());
        self.print_ln(b" >>");
        self.print_ln(b"startxref");
        self.print_int_ln(int(self.save_offset));
        self.print_ln(b"%%EOF");
        Ok(self.out)
    }

    /// `pdf_print_fw_int`: the last `w` digits.
    fn print_fw_int(&mut self, n: i64, w: usize) {
        let s = format!("{n:0w$}");
        let b = s.as_bytes();
        self.print(&b[b.len() - w..]);
    }

    /// `pdf_print_str` (pdfTeX §702).
    fn print_str(&mut self, s: &[u8]) {
        if s.is_empty() {
            self.print(b"()");
            return;
        }
        if s[0] == b'(' && s[s.len() - 1] == b')' {
            self.print(s);
            return;
        }
        let hex = s[0] == b'<'
            && s[s.len() - 1] == b'>'
            && s.len().is_multiple_of(2)
            && s[1..s.len() - 1].iter().all(u8::is_ascii_hexdigit);
        if hex {
            self.print(s);
        } else {
            self.pdf_out(b'(');
            self.print(s);
            self.pdf_out(b')');
        }
    }

    /// `@<Output pages tree@>` (pdfTeX §802–§803).
    fn output_pages_tree(&mut self, mut pages_tail: usize) {
        let a = self.objs.len(); // sys_obj_ptr+1
        let mut l = self.head[OBJ_TYPE_PAGES];
        let mut k = self.head[OBJ_TYPE_PAGE];
        let mut b = 0usize;
        loop {
            let mut i: i64 = 0;
            let mut c = 0usize;
            let is_root = self.objs[l].link == 0;
            loop {
                if !is_root {
                    if i % PAGES_TREE_KIDS_MAX == 0 {
                        self.last_pages = self.new_objnum();
                        if c == 0 {
                            c = self.last_pages;
                        }
                        self.objs[pages_tail].link = self.last_pages;
                        pages_tail = self.last_pages;
                        self.objs[self.last_pages].link = 0;
                        self.objs[self.last_pages].info = self.objs[l].info;
                    } else {
                        let lp = self.last_pages;
                        self.objs[lp].info += self.objs[l].info;
                    }
                }
                // §803
                self.begin_dict(l);
                self.print_ln(b"/Type /Pages");
                self.print(b"/Count ");
                self.print_int_ln(self.objs[l].info);
                if !is_root {
                    self.indirect_ln(b"Parent", self.last_pages);
                }
                self.print(b"/Kids [");
                let mut j: i64 = 0;
                loop {
                    self.print_int(int(k));
                    self.print(b" 0 R ");
                    k = self.objs[k].link;
                    j += 1;
                    if (l < a && j == self.objs[l].info)
                        || k == 0
                        || (k == b && b != 0)
                        || j == PAGES_TREE_KIDS_MAX
                    {
                        break;
                    }
                }
                self.remove_last_space();
                self.print_ln(b"]");
                if k == 0 {
                    k = self.head[OBJ_TYPE_PAGES];
                    self.head[OBJ_TYPE_PAGES] = 0;
                }
                self.end_dict();
                i += 1;
                l = self.objs[l].link;
                if l == c {
                    break;
                }
            }
            b = c;
            if l == 0 {
                break;
            }
        }
    }
}

/// The trees of `writefont.c` and `writeenc.c` while fonts are written.
#[derive(Default)]
struct FontTables {
    fds: Vec<Fd>,
    /// `fd_tree`: by font file, slant and extend.
    fd_tree: BTreeMap<(Vec<u8>, i32, i32), usize>,
    /// `fe_tree`: by encoding name.
    fe_tree: BTreeMap<Vec<u8>, Fe>,
    /// `fo_tree`: by TFM name.
    fo_tree: BTreeMap<Vec<u8>, Fo>,
}
