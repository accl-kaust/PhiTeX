//! pdfTeX's PDF backend: pages and forms from shipped boxes, objects,
//! links, destinations, threads, outlines, images and fonts;
//! byte-identical with pdfTeX 1.40.29.
//!
//! [`PdfState`] is `pdf_ship_out` (pdfTeX §750–§771) with
//! `pdf_hlist_out` and `pdf_vlist_out` (pdfTeX §729–§746) over
//! [`phitex_typeset`] nodes, the text state machine of pdfTeX §691–§693,
//! the object table and object streams of pdfTeX §694–§702, the
//! whatsits of pdfTeX §1342–§1620 (`ship.rs`, `marks.rs`), the colour
//! and matrix stacks of `utils.c`, the images of `writeimg.c`,
//! `writepng.c`, `writejpg.c` and `pdftoepdf.cc`, the fonts of
//! `writefont.c`, `writet1.c`, `writettf.c`, `writeenc.c`, `tounicode.c`
//! and `vfpacket.c`, and the end of the file (pdfTeX §794–§815). Streams
//! are compressed by `phitex_codecs`' port of zlib, the library pdfTeX
//! links.
//!
//! # When the TeX side calls what
//!
//! pdfTeX does part of its PDF work while TeX reads the document, part
//! when a page is shipped and part at the end; object numbers are given
//! in that order, so the TeX side calls the writer at the same moments:
//!
//! * **When the command is read** (`\immediate` or not): `\pdfobj`
//!   ([`PdfState::obj`], [`PdfState::obj_reserve`]), `\pdfxform`
//!   ([`PdfState::xform`]), `\pdfximage` ([`PdfState::ximage`]),
//!   `\pdfannot` and `\pdfstartlink` (their object number,
//!   [`PdfState::new_objnum`]), `\pdfoutline` ([`PdfState::outline`]),
//!   `\pdfcatalog`, `\pdfinfo`, `\pdfnames`, `\pdftrailer`,
//!   `\pdftrailerid` ([`PdfState::catalog`] and its siblings),
//!   `\pdfpageref` ([`PdfState::page_ref`]), `\pdfcolorstackinit`
//!   ([`PdfState::colorstack_init`]), `\pdffontattr`,
//!   `\pdfincludechars`, `\pdffontobjnum`, `\pdfmapline`, `\pdfmapfile`,
//!   `\pdfglyphtounicode`, `\pdfnobuiltintounicode`. With `\immediate`,
//!   `\pdfobj`, `\pdfxform` and `\pdfximage` are then written at once
//!   ([`PdfState::write_obj`], [`PdfState::write_xform`],
//!   [`PdfState::write_ximage`]).
//! * **At `\shipout`**: [`PdfState::ship_out_with`] (or
//!   [`PdfState::ship_out`] for lists without pdfTeX whatsits) walks the
//!   box; each whatsit is asked for its data ([`Whatsit`]) when it is
//!   met, so positions are those of the walk. The objects, forms and
//!   images the page refers to, its annotations, links, destinations and
//!   beads are written after its contents, as pdfTeX writes them.
//! * **At the end**: [`finish`] writes the fonts, the page tree,
//!   outlines, the name tree, threads, the catalog, the `/Info`
//!   dictionary and the cross-reference table or stream.
//!
//! # State and bytes
//!
//! The writer is a fold over the document: [`PdfState`] is everything
//! pdfTeX remembers (the object table and counters, fonts, images,
//! forms, destinations, outlines, threads, colour stacks, the map), and
//! it is `Clone` (cheaply: font files, images, forms' boxes and objects'
//! texts are shared) so the TeX side can snapshot it after any step and
//! read `\pdflastobj`, `\pdfpageref` and their siblings from the state
//! the step left. The bytes go to [`PdfBytes`], append-only: after a
//! step, [`PdfState::flush`] moves what it wrote there as one [`Chunk`],
//! with the numbers of the objects that begin in it. The state never
//! reads the bytes back; it only counts them, so object offsets are known
//! as objects begin. To redo the document from step `k`, continue from
//! the state cloned after `k` flushes with the bytes truncated to `k`
//! chunks ([`PdfBytes::truncate`]). [`finish`]`(state, &bytes)` appends
//! the end of the file. [`PdfState::finish`] is the same for a writer
//! never flushed.
//!
//! The parameters pdfTeX reads when it writes ([`PdfParams`]) can change
//! between calls ([`PdfState::params_mut`]): each is read when pdfTeX
//! reads it (`\pdfpagesattr` at the end, `\pdfpageattr` at each page, the
//! version and `\pdfobjcompresslevel` when the first byte is written).
//!
//! `\pdfrefxform` and `\pdfrefximage` are a whatsit followed by an empty
//! box of the form's or image's size (`phitex-tex`'s nodes): the box
//! moves the position past them; the writer places the form or image at
//! the whatsit as pdfTeX places its node.
//!
//! Not in this crate: PK (Type 3) fonts, OpenType (CFF) font files, TrueType subfonts,
//! `\pdfimageapplygamma`, JBIG2 images, the replacement of included PDF
//! files' Type 1C fonts, `\pdfsnapy`, `\pdfuniqueresname`.

#![allow(
    clippy::unreadable_literal,
    clippy::many_single_char_names,
    clippy::similar_names
)]

mod color;
mod digest;
pub mod enc;
mod finish;
pub use finish::{Chunk, PdfBytes, Tail, finish};
mod encodings;
mod epdffont;
mod font;
mod image;
mod macnames;
pub mod map;
mod marks;
#[doc(hidden)]
pub mod num;
mod objects;
/// (measure) `PHITEX_TEX_TIME`: per-thread phase clocks a finisher
/// reads out (the engine's phases and the writer's).
pub mod phase {
    use std::cell::Cell;
    /// The clocks.
    pub const N: usize = 24;
    thread_local! {
        static T: Cell<[u64; N]> = const { Cell::new([0; N]) };
    }
    /// Whether the clocks run.
    #[must_use]
    pub fn on() -> bool {
        static F: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *F.get_or_init(|| {
            std::env::var_os("PHITEX_TEX_TIME").is_some_and(|v| !v.is_empty() && v != "0")
        })
    }
    /// Clock `i` gains the time since `t0`.
    pub fn add(i: usize, t0: std::time::Instant) {
        if on() {
            let d = u64::try_from(t0.elapsed().as_nanos()).unwrap_or(0);
            T.with(|t| {
                let mut a = t.get();
                a[i] += d;
                t.set(a);
            });
        }
    }
    /// Clock `i` counts one (a count, not a time).
    pub fn bump(i: usize) {
        if on() {
            T.with(|t| {
                let mut a = t.get();
                a[i] += 1000;
                t.set(a);
            });
        }
    }
    /// The clocks, reset.
    #[must_use]
    pub fn take() -> [u64; N] {
        T.with(|t| t.replace([0; N]))
    }
}
mod replay;
pub use replay::{PageRecord, Part};
mod ship;
pub mod t1;
mod tounicode;
mod ttf;
mod vf;
mod whatsit;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use phitex_tfm::ScaledFont;
use phitex_typeset::arith::Scaled;
use phitex_typeset::fonts::FontId;
use phitex_typeset::node::BoxNode;

pub use crate::image::{ImageSpec, PageBox, XImageInfo};
use crate::map::MapEntry;
use crate::num::ONE_HUNDRED_INCH;
pub use crate::objects::ObjBody;
pub use crate::whatsit::{
    Action, ColorStackCmd, DestKind, Ident, LiteralMode, Whatsit, WhatsitSource,
};

/// The texts of a shipped list's whatsits for [`PdfState::ship_out`]:
/// the node's handle and whether leaders are being output, answered
/// with a `\special`'s text (or `None` for a whatsit not written).
pub type Whatsits<'a> = dyn FnMut(u32, bool) -> Option<Vec<u8>> + 'a;

/// Why output failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PdfError {
    /// "Huge page cannot be shipped out" (tex.web §641, pdfTeX §751).
    HugePage,
    /// A font that cannot be written (a fatal `pdftex_fail` in pdfTeX).
    Font(String),
    /// A fatal `pdf_error` or `pdftex_fail`: its context and message.
    Pdf(String, String),
    /// Something this crate does not do yet.
    Unsupported(String),
}

impl std::fmt::Display for PdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PdfError::HugePage => write!(f, "Huge page cannot be shipped out"),
            PdfError::Font(m) => write!(f, "pdfTeX error (font): {m}"),
            PdfError::Pdf(t, m) if t.is_empty() => write!(f, "pdfTeX error: {m}"),
            PdfError::Pdf(t, m) => write!(f, "pdfTeX error ({t}): {m}"),
            PdfError::Unsupported(m) => write!(f, "not supported: {m}"),
        }
    }
}

impl std::error::Error for PdfError {}

/// `pdf_error(t, p)`.
fn pdf_error(t: &str, p: &str) -> PdfError {
    PdfError::Pdf(t.to_owned(), p.to_owned())
}

/// The parameters pdfTeX reads when it writes: `\pdf...` registers and
/// token lists, `\hoffset`, `\voffset`, `\mag`, and what the system
/// gives (the date, the banner, the file name).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PdfParams {
    /// `\pdfcompresslevel` (0–9).
    pub compress_level: i32,
    /// `\pdfobjcompresslevel` (0–3).
    pub obj_compress_level: i32,
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
    /// `\pdflinkmargin`.
    pub link_margin: Scaled,
    /// `\pdfdestmargin`.
    pub dest_margin: Scaled,
    /// `\pdfthreadmargin`.
    pub thread_margin: Scaled,
    /// `\pdfpageattr`, as `tokens_to_string` prints it.
    pub page_attr: Option<Vec<u8>>,
    /// `\pdfpagesattr`.
    pub pages_attr: Option<Vec<u8>>,
    /// `\pdfpageresources`.
    pub page_resources: Option<Vec<u8>>,
    /// `\pdfgentounicode`.
    pub gen_tounicode: i32,
    /// `\pdfdraftmode`.
    pub draftmode: i32,
    /// `\pdfimagehicolor`.
    pub image_hicolor: i32,
    /// `\pdfimageapplygamma`.
    pub image_apply_gamma: i32,
    /// `\pdfgamma`.
    pub gamma: i32,
    /// `\pdfimagegamma`.
    pub image_gamma: i32,
    /// `\pdfinclusioncopyfonts`.
    pub inclusion_copy_fonts: i32,
    /// `\pdfsuppressptexinfo`.
    pub suppress_ptex_info: i32,
    /// `\pdfptexuseunderscore`.
    pub ptex_use_underscore: i32,
    /// `\pdfomitinfodict`.
    pub omit_info_dict: i32,
    /// `\pdfomitprocset`.
    pub omit_procset: i32,
    /// `\pdfomitcharset`.
    pub omit_charset: i32,
    /// `\pdfinfoomitdate`.
    pub info_omit_date: i32,
    /// `\pdfsuppresswarningpagegroup`.
    pub suppress_warning_page_group: i32,
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
            obj_compress_level: 0,
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
            link_margin: 0,
            dest_margin: 0,
            thread_margin: 0,
            page_attr: None,
            pages_attr: None,
            page_resources: None,
            gen_tounicode: 0,
            draftmode: 0,
            image_hicolor: 1,
            image_apply_gamma: 0,
            gamma: 1000,
            image_gamma: 2200,
            inclusion_copy_fonts: 0,
            suppress_ptex_info: 0,
            ptex_use_underscore: 0,
            omit_info_dict: 0,
            omit_procset: 0,
            omit_charset: 0,
            info_omit_date: 0,
            suppress_warning_page_group: 0,
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

/// An expanded font (`\pdffontexpand`): the font it was made from, by how
/// many thousandths, and whether it is auto-expanded (`autoexpand`: it
/// shares the base font's object and is drawn with a text matrix).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Expansion {
    /// `pdf_font_blink`.
    pub base: FontId,
    /// `pdf_font_expand_ratio`.
    pub ratio: i32,
    /// `pdf_font_auto_expand`.
    pub auto: bool,
}

/// A virtual font (`do_vf`): its local fonts, loaded by the TeX side
/// when the writer asks (`vf_def_font`: `tfm_lookup`, else
/// `read_font_info`), and its character packets.
#[derive(Clone, Debug, Default)]
pub struct VirtualFont {
    /// The local fonts in the file's order: the font number `k` of the
    /// `fnt_def` and the TeX font it is.
    pub fonts: Vec<(u32, FontId)>,
    /// Each character's DVI program, as in the `.vf` file.
    pub packets: BTreeMap<u8, Vec<u8>>,
}

/// The caller's fonts, by TeX's internal font number (`null_font` is 0).
pub trait FontSource {
    /// The metrics at the font's size (`font_size`, `char_width`, ...).
    fn metrics(&self, f: FontId) -> &ScaledFont;
    /// `font_name`: the TFM name, as the map file names it.
    fn name(&self, f: FontId) -> &[u8];
    /// The font's map entry (`pdftex.map`), if any.
    fn map_entry(&self, f: FontId) -> Option<MapEntry>;
    /// The bytes of the map entry's font file (`.pfb`, `.pfa`, `.ttf`).
    fn font_file(&self, f: FontId) -> Option<Vec<u8>>;
    /// The bytes of the map entry's encoding file (`.enc`).
    fn encoding_file(&self, f: FontId) -> Option<Vec<u8>>;
    /// A file by name, as kpathsea finds it (font files and encodings of
    /// map lines given with `\pdfmapline`).
    fn file(&self, _name: &[u8]) -> Option<Vec<u8>> {
        None
    }
    /// The font's expansion, if it is an expanded font.
    fn expansion(&self, _f: FontId) -> Option<Expansion> {
        None
    }
    /// The font's virtual font, its local fonts loaded now (`do_vf`), if
    /// it has a `.vf` file.
    fn virtual_font(&self, _f: FontId) -> Option<VirtualFont> {
        None
    }
    /// `lookup_fontmap` in the default map: the entry (`ps_tree`, the
    /// first of the map file's lines for a TFM name that is a Type 1 font
    /// with a font file included) of the PostScript name with slant and
    /// extend, for the font replacement of included PDF files.
    fn map_by_ps_name(&self, _ps_name: &[u8], _slant: i32, _extend: i32) -> Option<MapEntry> {
        None
    }
    /// `pdf_read_dummy_font`: the font `\pdfspacefont` names
    /// (`pdftexspace`), loaded now at its design size.
    fn space_font(&self) -> Option<FontId> {
        None
    }
    /// The number in font `f`'s name (`/F<n>`) where it is not `f`
    /// (pdfTeX's numbering, which a writer of pages apart learns when
    /// they are linked).
    fn tag(&self, _f: FontId) -> Option<FontId> {
        None
    }
}

// object types (pdfTeX §695)
const OBJ_TYPE_OTHERS: usize = 0;
const OBJ_TYPE_PAGE: usize = 1;
const OBJ_TYPE_PAGES: usize = 2;
const OBJ_TYPE_FONT: usize = 3;
const OBJ_TYPE_OUTLINE: usize = 4;
const OBJ_TYPE_DEST: usize = 5;
const OBJ_TYPE_STRUCT_DEST: usize = 6;
const OBJ_TYPE_OBJ: usize = 7;
const OBJ_TYPE_XFORM: usize = 8;
const OBJ_TYPE_XIMAGE: usize = 9;
const OBJ_TYPE_THREAD: usize = 10;
const HEAD_TAB_MAX: usize = 10;
/// `pages_tree_kids_max` (pdfTeX §695).
const PAGES_TREE_KIDS_MAX: i64 = 6;
/// `name_tree_kids_max`.
const NAME_TREE_KIDS_MAX: usize = 6;
/// `pdf_os_max_objs`.
const PDF_OS_MAX_OBJS: usize = 100;
/// `max_dimen`.
const MAX_DIMEN: Scaled = 0o7777777777;
/// `billion` (tex.web §625).
const BILLION: f64 = 1_000_000_000.0;
/// `pdf_max_link_level`.
const PDF_MAX_LINK_LEVEL: usize = 10;

/// An `obj_entry` (pdfTeX §694–§695).
#[derive(Clone, Copy, Debug, Default)]
struct Obj {
    info: i64,
    link: usize,
    /// Negative until written (-2 fresh, -1 scheduled), then the byte
    /// offset, or the object stream's number. While the document is
    /// written a direct object's offset is 0 here: its place is its
    /// chunk's ([`Chunk::objs`](crate::Chunk)), known only when
    /// [`Tail::link`](crate::Tail::link) places the chunks.
    offset: i64,
    /// `obj_os_idx`: the index in its object stream, -1 if not in one.
    os_idx: i32,
}

/// `pdf_font_type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FontType {
    New,
    Virtual,
    Real,
}

/// What pdfTeX keeps per TeX font (pdfTeX §708, `font_used`), with the
/// caller's data for it.
#[derive(Clone)]
struct FontState {
    name: Vec<u8>,
    metrics: ScaledFont,
    map: Option<MapEntry>,
    file: Option<Arc<[u8]>>,
    enc: Option<Arc<[u8]>>,
    used: bool,
    /// `pdf_font_size`.
    size: Scaled,
    /// `pdf_font_num`.
    num: i64,
    /// `pdf_char_used`.
    chars: [bool; 256],
    /// `pdf_font_attr`.
    attr: Vec<u8>,
    /// `pdf_font_nobuiltin_tounicode`.
    nobuiltin_tounicode: bool,
    /// `pdf_font_has_space_char`.
    has_space: bool,
    /// `pdf_font_type`.
    kind: FontType,
    /// The virtual font, once read.
    vf: Option<VirtualFont>,
    /// The font's expansion.
    expand: Option<Expansion>,
}

impl std::fmt::Debug for FontState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontState")
            .field("name", &self.name)
            .field("metrics", &self.metrics)
            .field("map", &self.map)
            .field("file", &digest::opt(self.file.as_ref()))
            .field("enc", &digest::opt(self.enc.as_ref()))
            .field("used", &self.used)
            .field("size", &self.size)
            .field("num", &self.num)
            .field("chars", &self.chars)
            .field("attr", &self.attr)
            .field("nobuiltin_tounicode", &self.nobuiltin_tounicode)
            .field("has_space", &self.has_space)
            .field("kind", &self.kind)
            .field("vf", &self.vf)
            .field("expand", &self.expand)
            .finish()
    }
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
    /// `get_font_auto_expand_ratio`.
    fn auto_expand_ratio(&self) -> i32 {
        self.expand.filter(|x| x.auto).map_or(0, |x| x.ratio)
    }
}

/// A rectangle on the page (`pdf_left` ... `pdf_bottom`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Rect {
    left: Scaled,
    top: Scaled,
    right: Scaled,
    bottom: Scaled,
}

/// An `i64` from a count of objects or bytes.
fn int(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// The object stream being filled (pdfTeX §680's `pdf_os_*`).
#[derive(Clone, Debug, Default)]
struct ObjStm {
    /// `pdf_os_enable`.
    enable: bool,
    /// `pdf_os_mode`.
    mode: bool,
    /// `pdf_os_buf` up to `pdf_ptr`.
    buf: Vec<u8>,
    /// `pdf_os_cur_objnum`.
    cur: usize,
    /// `pdf_os_objnum` and `pdf_os_objoff` up to `pdf_os_objidx`.
    list: Vec<(usize, usize)>,
}

/// The data of raw objects, forms, images and marks (`obj_aux` and the
/// structures it points to).
#[derive(Clone, Debug)]
struct Aux {
    /// `\pdfobj` data (`obj_data_ptr`).
    raw: digest::TMap<usize, Arc<objects::RawObj>>,
    /// `\pdfxform` data.
    xforms: digest::TMap<usize, objects::XForm>,
    /// `\pdfximage` data.
    ximages: digest::TMap<usize, image::XImage>,
    /// `obj_annot_ptr`: annotations' and links' rectangles and data.
    marks: digest::TMap<usize, marks::Mark>,
    /// `obj_dest_ptr`: destinations met.
    dests: digest::TMap<usize, marks::Dest>,
    /// Outline entries.
    outlines: digest::TMap<usize, marks::Outline>,
    /// `obj_thread_first`.
    thread_first: digest::TMap<usize, usize>,
    /// Beads.
    beads: digest::TMap<usize, marks::Bead>,
    /// `obj_aux` of a page object: the page was shipped.
    pages_made: BTreeSet<usize>,
    /// Named objects' names (`obj_info` < 0 is a string).
    names: digest::TMap<usize, Vec<u8>>,
}

impl Default for Aux {
    fn default() -> Aux {
        Aux {
            raw: digest::TMap::new("raw"),
            xforms: digest::TMap::new("xforms"),
            ximages: digest::TMap::new("ximages"),
            marks: digest::TMap::new("marks"),
            dests: digest::TMap::new("dests"),
            outlines: digest::TMap::new("outlines"),
            thread_first: digest::TMap::new("thread_first"),
            beads: digest::TMap::new("beads"),
            pages_made: BTreeSet::new(),
            names: digest::TMap::new("names"),
        }
    }
}

impl PdfState {
    /// Brings the tracked collections' sums up to date.
    pub(crate) fn refresh_digests(&mut self) {
        let a = &mut self.aux;
        a.raw.refresh();
        a.xforms.refresh();
        a.ximages.refresh();
        a.marks.refresh();
        a.dests.refresh();
        a.outlines.refresh();
        a.thread_first.refresh();
        a.beads.refresh();
        a.names.refresh();
        self.objs.refresh();
        self.index.refresh();
        self.fonts.refresh();
        self.epdf_fds.refresh();
        self.map_over.refresh();
        self.map_add.refresh();
        self.glyph_unicode.refresh();
        self.dest_names.refresh();
        self.epdf_docs.refresh();
    }
}

/// The writer before the split: the state and its bytes in one.
pub type PdfWriter = PdfState;

/// The PDF writer's state: pdfTeX's state across pages, everything but
/// the bytes written (see [`PdfBytes`]). Cloning it is cheap: the large
/// immutable parts (font files, images, forms' boxes, objects' texts)
/// are shared.
#[derive(Clone, Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct PdfState {
    p: PdfParams,
    /// The bytes written since the last [`PdfState::flush`].
    out: Vec<u8>,
    /// Where `out` begins in the file: 0 while the document is written
    /// (offsets are within the step's chunk, kept by the chunk), the
    /// bytes before when [`Tail::link`](crate::Tail::link) writes the
    /// cross-reference section (`linking`).
    base: usize,
    linking: bool,
    /// The chunks flushed (non-empty steps), the ordinal of the next one.
    flushes: usize,
    /// The objects begun in `out`, with their offsets in it.
    chunk_objs: Vec<(usize, usize)>,
    /// The stream being compressed (`zip_write_state=zip_writing`).
    zip: Option<Vec<u8>>,
    stream_length_offset: usize,
    save_offset: usize,
    /// The last page's content stream, uncompressed, and its size.
    page_shown: (Vec<u8>, Scaled, Scaled),
    objs: digest::TVec<Obj>,
    /// `obj_ptr`: the objects counted for the user (object streams are
    /// not).
    obj_ptr: usize,
    head: [usize; HEAD_TAB_MAX + 1],
    /// `avl_find_obj`: objects by type and identifier.
    index: digest::TMap<(usize, Ident), usize>,
    aux: Aux,
    os: ObjStm,
    version_written: bool,
    /// `fixed_pdf_major_version`, `fixed_pdf_minor_version`.
    fixed_major: i32,
    fixed_minor: i32,
    /// `fixed_pdf_objcompresslevel`.
    fixed_ocl: i32,
    fixed_draftmode: i32,
    fixed_image_hicolor: i32,
    fixed_inclusion_copy_font: i32,
    initialized: bool,
    /// `fixed_decimal_digits`.
    dd: i32,
    min_bp_val: Scaled,
    total_pages: i64,
    last_pages: usize,
    last_page: usize,
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
    /// `pdf_cur_Tm_a`.
    tm_a: i32,
    cur_page_width: Scaled,
    cur_page_height: Scaled,
    cur_s: i32,
    doing_leaders: bool,
    /// `is_shipping_page`.
    shipping_page: bool,
    /// `pdf_cur_form` and the form's size.
    cur_form: usize,
    xform_height: Scaled,
    xform_depth: Scaled,
    /// The resource lists of this page or form.
    font_list: Vec<FontId>,
    obj_list: Vec<usize>,
    xform_list: Vec<usize>,
    ximage_list: Vec<usize>,
    text_procset: bool,
    image_procset: i32,
    /// `pdf_page_group_val`.
    page_group_val: usize,
    /// The mark lists of this page.
    annot_list: Vec<usize>,
    link_list: Vec<usize>,
    dest_list: Vec<usize>,
    bead_list: Vec<usize>,
    link_stack: Vec<marks::LinkLevel>,
    thread: marks::ThreadState,
    /// `gen_faked_interword_space`.
    faked_space: bool,
    /// `gen_running_link`.
    running_link: bool,
    /// `pdf_dummy_font`.
    dummy_font: FontId,
    /// The first fatal error met while walking a list.
    error: Option<PdfError>,
    stacks: color::Stacks,
    fonts: digest::TMap<FontId, Arc<FontState>>,
    /// A font known to be in `fonts` (the last one `load_font` saw):
    /// the per-character check is a compare.
    last_loaded: Option<FontId>,
    /// `divide_scaled(w, size, 4)` of characters' widths, direct-mapped
    /// by width and size (a pure function: a line repeats its letters).
    adv_memo: Box<[(Scaled, Scaled, i32, Scaled); 64]>,
    /// `fd_tree`'s descriptors of included PDF files' fonts.
    epdf_fds: digest::TMap<epdffont::FdKey, epdffont::EpdfFd>,
    /// `\pdfmapline` and `\pdfmapfile` entries: replacing (`=`) or
    /// deleting (`-`) ones, and those added only where the map has none
    /// (`+`).
    map_over: digest::TMap<Vec<u8>, Option<MapEntry>>,
    map_add: digest::TMap<Vec<u8>, MapEntry>,
    /// `glyph_unicode_tree`.
    glyph_unicode: digest::TMap<Vec<u8>, tounicode::Uni>,
    /// `fixed_gen_tounicode`.
    fixed_gen_tounicode: i32,
    // the font files' state across fonts
    subset_tags: BTreeSet<[u8; 6]>,
    last_arg_other_subr3: i32,
    // `\pdfinfo` and friends, concatenated
    info_toks: Option<Vec<u8>>,
    catalog_toks: Option<Vec<u8>>,
    catalog_openaction: usize,
    names_toks: Option<Vec<u8>>,
    trailer_toks: Option<Vec<u8>>,
    trailer_id_toks: Option<Vec<u8>>,
    /// `pdf_first_outline`, `pdf_last_outline`, `pdf_parent_outline`.
    first_outline: usize,
    last_outline: usize,
    parent_outline: usize,
    /// `pdf_obj_count`, `pdf_xform_count`, `pdf_ximage_count`.
    obj_count: i64,
    xform_count: i64,
    ximage_count: i64,
    /// `dest_names`.
    dest_names: digest::TVec<(Vec<u8>, usize)>,
    /// The included PDF files (`pdfDocuments`).
    epdf_docs: digest::TVec<image::EpdfDoc>,
    /// `transparent_page_group` and whether it was written (`writepng.c`).
    png_group: usize,
    png_group_written: bool,
    /// The map was read (a font was looked up), so `\pdfmapline`
    /// without a prefix no longer drops the default map.
    map_read: bool,
    /// The default map was dropped (`mitem->line = NULL`).
    no_default_map: bool,
    /// `vf_cur_s`.
    vf_cur_s: i32,
    /// `pdf_last_byte`, which `pdf_newline` reads.
    last_byte: u8,
    /// A page's walk being recorded ([`PdfState::ship_recorded`]).
    rec: Option<Box<replay::Recording>>,
}

impl PdfState {
    /// A writer; nothing is written before the first page or object.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn new(params: PdfParams) -> PdfState {
        PdfState {
            p: params,
            out: Vec::new(),
            base: 0,
            linking: false,
            flushes: 0,
            chunk_objs: Vec::new(),
            zip: None,
            stream_length_offset: 0,
            save_offset: 0,
            page_shown: (Vec::new(), 0, 0),
            objs: {
                let mut o = digest::TVec::new("objs");
                o.push(Obj::default());
                o
            },
            obj_ptr: 0,
            head: [0; HEAD_TAB_MAX + 1],
            index: digest::TMap::new("index"),
            aux: Aux::default(),
            os: ObjStm::default(),
            version_written: false,
            fixed_major: 1,
            fixed_minor: 4,
            fixed_ocl: 0,
            fixed_draftmode: 0,
            fixed_image_hicolor: 1,
            fixed_inclusion_copy_font: 0,
            initialized: false,
            dd: 3,
            min_bp_val: 0,
            total_pages: 0,
            last_pages: 0,
            last_page: 0,
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
            tm_a: 0,
            cur_page_width: 0,
            cur_page_height: 0,
            cur_s: -1,
            doing_leaders: false,
            shipping_page: false,
            cur_form: 0,
            xform_height: 0,
            xform_depth: 0,
            font_list: Vec::new(),
            obj_list: Vec::new(),
            xform_list: Vec::new(),
            ximage_list: Vec::new(),
            text_procset: false,
            image_procset: 0,
            page_group_val: 0,
            annot_list: Vec::new(),
            link_list: Vec::new(),
            dest_list: Vec::new(),
            bead_list: Vec::new(),
            link_stack: Vec::new(),
            thread: marks::ThreadState::default(),
            faked_space: false,
            running_link: true,
            dummy_font: 0,
            error: None,
            stacks: color::Stacks::default(),
            epdf_fds: digest::TMap::new("epdf_fds"),
            fonts: digest::TMap::new("fonts"),
            last_loaded: None,
            adv_memo: Box::new([(0, 0, 0, 0); 64]),
            map_over: digest::TMap::new("map_over"),
            map_add: digest::TMap::new("map_add"),
            glyph_unicode: digest::TMap::new("glyph_unicode"),
            fixed_gen_tounicode: 0,
            subset_tags: BTreeSet::new(),
            last_arg_other_subr3: 3,
            info_toks: None,
            catalog_toks: None,
            catalog_openaction: 0,
            names_toks: None,
            trailer_toks: None,
            trailer_id_toks: None,
            first_outline: 0,
            last_outline: 0,
            parent_outline: 0,
            obj_count: 0,
            xform_count: 0,
            ximage_count: 0,
            dest_names: digest::TVec::new("dest_names"),
            epdf_docs: digest::TVec::new("epdf_docs"),
            png_group: 0,
            png_group_written: false,
            map_read: false,
            no_default_map: false,
            vf_cur_s: 0,
            last_byte: 0,
            rec: None,
        }
    }

    /// The parameters, to change as TeX's registers change.
    pub fn params_mut(&mut self) -> &mut PdfParams {
        &mut self.p
    }

    /// What the last page shipped shows: its content stream
    /// (uncompressed), width and height; no object numbers.
    #[must_use]
    pub fn last_page_shown(&self) -> (&[u8], Scaled, Scaled) {
        (&self.page_shown.0, self.page_shown.1, self.page_shown.2)
    }

    /// The number of pages shipped.
    #[must_use]
    pub fn total_pages(&self) -> i64 {
        self.total_pages
    }

    /// Records the first fatal error met in a list walk.
    fn fail(&mut self, e: PdfError) {
        if self.error.is_none() {
            self.error = Some(e);
        }
    }

    // ---- output (pdfTeX §680–§686) ----

    fn buf(&mut self) -> &mut Vec<u8> {
        if self.os.mode {
            return &mut self.os.buf;
        }
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

    /// `pdf_indirect`.
    fn indirect(&mut self, key: &[u8], o: usize) {
        self.pdf_out(b'/');
        self.print(key);
        self.pdf_out(b' ');
        self.print_int(int(o));
        self.print(b" 0 R");
    }

    /// `pdf_indirect_ln`.
    fn indirect_ln(&mut self, key: &[u8], o: usize) {
        self.indirect(key, o);
        self.pdf_out(b'\n');
    }

    /// `pdf_int_entry_ln`.
    fn int_entry_ln(&mut self, key: &[u8], v: i64) {
        self.pdf_out(b'/');
        self.print(key);
        self.pdf_out(b' ');
        self.print_int_ln(v);
    }

    /// `remove_last_space`.
    fn remove_last_space(&mut self) {
        if self.buf().last() == Some(&b' ') {
            self.buf().pop();
        }
    }

    /// `pdf_offset`.
    fn offset(&self) -> usize {
        self.base + self.out.len()
    }

    /// Where the content stream being written is: the bytes of it so
    /// far, uncompressed.
    fn content_pos(&self) -> usize {
        match &self.zip {
            Some(z) => z.len(),
            None => self.offset().saturating_sub(self.save_offset),
        }
    }

    /// `pdf_begin_stream`.
    fn begin_stream(&mut self) {
        self.print_ln(b"/Length           ");
        self.stream_length_offset = self.offset() - 11;
        self.last_byte = 0;
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
            let t0 = std::time::Instant::now();
            let z = compress_memo(&data, self.p.compress_level);
            phase::add(11, t0);
            self.out.extend_from_slice(&z);
            z.len()
        } else {
            self.offset() - self.save_offset
        };
        let digits = length.to_string();
        let at = self.stream_length_offset - self.base;
        self.out[at..at + digits.len()].copy_from_slice(digits.as_bytes());
        self.end_raw_stream();
    }

    /// The end of a stream whose `/Length` its writer printed
    /// (`pdf_end_stream` with `pdf_seek_write_length` false).
    fn end_raw_stream(&mut self) {
        self.pdf_out(b'\n');
        self.print_ln(b"endstream");
        self.end_obj();
    }

    /// `check_pdfversion` (pdfTeX §683): the header.
    fn check_pdfversion(&mut self) {
        if self.version_written {
            return;
        }
        self.version_written = true;
        if self.p.major_version < 1 {
            self.p.major_version = 1;
        }
        if !(0..=9).contains(&self.p.minor_version) {
            self.p.minor_version = 4;
        }
        self.fixed_major = self.p.major_version;
        self.fixed_minor = self.p.minor_version;
        self.fixed_image_hicolor = self.p.image_hicolor.clamp(0, 1);
        self.fixed_ocl = self.p.obj_compress_level.clamp(0, 3);
        self.fixed_draftmode = self.p.draftmode.clamp(0, 1);
        self.fixed_inclusion_copy_font = self.p.inclusion_copy_fonts.clamp(0, 1);
        if (self.fixed_major > 1 || self.fixed_minor >= 5) && self.fixed_ocl > 0 {
            self.os.enable = true;
        } else {
            // "Object streams disabled now."
            self.fixed_ocl = 0;
            self.os.enable = false;
        }
        // fix_pdf_draftmode
        if self.fixed_draftmode > 0 {
            self.p.compress_level = 0;
            self.fixed_ocl = 0;
        }
        self.print(b"%PDF-");
        self.print_int(i64::from(self.fixed_major));
        self.pdf_out(b'.');
        self.print_int_ln(i64::from(self.fixed_minor));
        self.pdf_out(b'%');
        self.print(&[208, 212, 197, 216]);
        self.pdf_out(b'\n');
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

    /// `pdf_print_toks_ln`: a text and a newline, nothing if empty.
    fn print_toks_ln(&mut self, s: &[u8]) {
        if !s.is_empty() {
            self.print_ln(s);
        }
    }
}

/// `compress(data, level)`, memoized: a page whose content stream an
/// earlier write already compressed (the exact writer's page steps
/// write each page ahead, in parallel, before the finish writes it in
/// order) is not compressed again. A pure function's memo, keyed by
/// two independent hashes of the data, its length and the level.
pub(crate) fn compress_memo(data: &[u8], level: i32) -> Vec<u8> {
    use std::hash::{Hash, Hasher};
    use std::sync::{Mutex, OnceLock, PoisonError};
    type Memo = Mutex<(
        std::collections::HashMap<(u64, u64, usize, i32), Arc<[u8]>>,
        usize,
    )>;
    static MEMO: OnceLock<Memo> = OnceLock::new();
    /// The bytes kept before the memo starts again.
    const ROOM: usize = 256 << 20;
    thread_local! {
        /// The compressor's buffers, kept from stream to stream.
        static SCRATCH: std::cell::RefCell<phitex_codecs::deflate::Scratch> =
            std::cell::RefCell::default();
    }
    let compress = |data: &[u8]| {
        SCRATCH.with(|sc| match sc.try_borrow_mut() {
            Ok(mut sc) => phitex_codecs::deflate::compress_with(&mut sc, data, level),
            Err(_) => phitex_codecs::deflate::compress(data, level),
        })
    };
    if data.len() < 512 {
        return compress(data);
    }
    let h = |seed: u64| {
        let mut s = std::hash::DefaultHasher::new();
        seed.hash(&mut s);
        data.hash(&mut s);
        s.finish()
    };
    let key = (h(0), h(0x9e37_79b9_7f4a_7c15), data.len(), level);
    let memo = MEMO.get_or_init(|| Mutex::new((std::collections::HashMap::new(), 0)));
    if let Some(z) = memo
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .0
        .get(&key)
    {
        return z.to_vec();
    }
    let z = compress(data);
    let mut m = memo.lock().unwrap_or_else(PoisonError::into_inner);
    if m.1 + z.len() > ROOM {
        m.0.clear();
        m.1 = 0;
    }
    m.1 += z.len();
    m.0.insert(key, Arc::from(z.as_slice()));
    z
}

/// `substr_of_str(s, t)`: whether `s` occurs in `t` (not at its very
/// end: pdfTeX's loop stops one short).
fn substr_of_str(s: &[u8], t: &[u8]) -> bool {
    if t.len() < s.len() {
        return false;
    }
    (0..t.len() - s.len()).any(|k| t[k..].starts_with(s))
}
