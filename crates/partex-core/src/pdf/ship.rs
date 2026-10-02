//! pdfTeX parts 32c and 32f: shipping boxes out as PDF pages and forms
//! (pdfTeX §727–§791), and the page marks of part 53 (annotations,
//! links, destinations) placed while shipping.
//!
//! The walk is `pdf_hlist_out`/`pdf_vlist_out` over typed nodes. It runs in
//! the engine because shipping is observable: objects are numbered as they
//! are met (fonts, links, pages), `\pdfsavepos` gives `\write`s positions,
//! and late literals expand macros.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

use partex_engine::node::{Action, BoxNode, Dims, Leaders, Node, PdfId, PdfWhatsit, Whatsit};

use super::draw::Draw;
use super::enc::GlyphNames;
use super::objtab::{
    Aux, Id, OBJ_TYPE_DEST, OBJ_TYPE_OTHERS, OBJ_TYPE_PAGE, OBJ_TYPE_PAGES, OBJ_TYPE_STRUCT_DEST,
    OBJ_TYPE_THREAD,
};
use super::out::{TEN_POW, round_xn_over_d};
use super::val::{Element, SHIPPING, SHIPPING_READS, VMap, VTab, Val};
use super::{DIRECT_ALWAYS, DIRECT_PAGE, ONE_BP, ONE_HUNDRED_BP, SCAN_SPECIAL, SET_ORIGIN};
use crate::arith::Scaled;
use crate::dvi::SetGlue;
use crate::fontmap::{F_PK, MapEntry};
use crate::host::Host;
use crate::scan::MAX_DIMEN;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;
use crate::web::*;

/// A rule dimension not given.
const RUNNING: Scaled = super::ext::RUNNING;

/// `pages_tree_kids_max`.
pub(crate) const PAGES_TREE_KIDS_MAX: i32 = 6;
/// `pdf_max_link_level`.
const PDF_MAX_LINK_LEVEL: usize = 10;

/// What pdfTeX keeps per font for PDF output (`font_used`,
/// `pdf_font_size`, `pdf_font_num`, `pdf_char_used`, …).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PdfFont {
    pub used: bool,
    pub size: Scaled,
    /// The font object, or minus the font sharing it.
    pub num: i32,
    pub has_space: bool,
    pub chars: [u64; 4],
    /// `pdf_font_map`: looked up (`Some`), with the entry if any.
    #[expect(clippy::option_option, reason = "not looked up, or none")]
    pub map: Option<Option<Arc<MapEntry>>>,
    /// `pdf_font_type`.
    pub font_type: super::vf::FontType,
}

partex_engine::persist_struct!(PdfFont {
    used,
    size,
    num,
    has_space,
    chars,
    map,
    font_type
});

/// What the PDF writer keeps of each font, by slot: the `PDF_FONTS`
/// field (a table of shared chunks, each font with its version, its
/// glyphs used included: §7.16's `Glyphs`). Its `Hash` is empty: the
/// state hash takes it apart (a machine's `MCell::Font` cells hold it,
/// `machine.rs`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PdfFonts(pub VTab<PdfFont>);

impl core::hash::Hash for PdfFonts {
    fn hash<H: core::hash::Hasher>(&self, _h: &mut H) {}
}

impl<'a> IntoIterator for &'a PdfFonts {
    type Item = &'a PdfFont;
    type IntoIter = alloc::boxed::Box<dyn Iterator<Item = &'a PdfFont> + 'a>;
    fn into_iter(self) -> Self::IntoIter {
        alloc::boxed::Box::new(self.0.iter())
    }
}

impl core::ops::Deref for PdfFonts {
    type Target = VTab<PdfFont>;
    fn deref(&self) -> &VTab<PdfFont> {
        &self.0
    }
}

impl core::ops::DerefMut for PdfFonts {
    fn deref_mut(&mut self) -> &mut VTab<PdfFont> {
        &mut self.0
    }
}

/// Every field but the glyphs used, which are the ships' appends
/// (`Ship::glyphs`), made whole at the job's end.
impl Element for PdfFont {
    fn element_version(&self) -> u128 {
        partex_ssa::Version::of(self).0
    }
}

impl partex_engine::persist::Persist for PdfFonts {
    fn save(&self, s: &mut partex_engine::persist::Saver) {
        self.0.save(s);
    }
    fn load(l: &mut partex_engine::persist::Loader) -> Option<Self> {
        Some(Self(partex_engine::persist::Persist::load(l)?))
    }
}

/// Hashes all but `chars`, the glyphs used, which the state hash takes
/// apart (a machine's `Glyphs` cell, `machine.rs`).
impl core::hash::Hash for PdfFont {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        let Self {
            used,
            size,
            num,
            has_space,
            chars: _,
            map,
            font_type,
        } = self;
        (used, size, num, has_space, map, font_type).hash(h);
    }
}

impl PdfFont {
    pub(crate) fn mark(&mut self, c: u8) {
        self.chars[usize::from(c >> 6)] |= 1 << (c & 63);
    }

    pub(crate) fn marked(&self, c: u8) -> bool {
        self.chars[usize::from(c >> 6)] & (1 << (c & 63)) != 0
    }
}

/// An open link (`pdf_link_stack_record`): the start node's dimensions,
/// what it links, and the annotation its rectangle is in.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct LinkLevel {
    level: i32,
    dims: Dims,
    attr: Option<partex_engine::node::Tokens>,
    action: Arc<Action>,
    /// `ref_link_node`: the annotation whose rectangle `\pdfendlink`
    /// finishes.
    objnum: i32,
}

partex_engine::persist_struct!(LinkLevel {
    level,
    dims,
    attr,
    action,
    objnum
});

/// A rectangle (`pdf_left`, `pdf_top`, `pdf_right`, `pdf_bottom`), in
/// DVI coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Rect {
    pub left: Scaled,
    pub top: Scaled,
    pub right: Scaled,
    pub bottom: Scaled,
}

partex_engine::persist_struct!(Rect {
    left,
    top,
    right,
    bottom
});

/// An annotation or link placed on the current page (`obj_annot_ptr`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Mark {
    pub rect: Rect,
    /// The annotation's text, or the link's `attr`.
    pub data: Option<partex_engine::node::Tokens>,
    pub action: Option<Arc<Action>>,
}

partex_engine::persist_struct!(Mark { rect, data, action });

/// A destination placed on a page (`obj_dest_ptr`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Dest {
    pub rect: Rect,
    pub kind: u8,
    pub zoom: Option<i32>,
    pub struct_num: Option<i32>,
    pub named: bool,
}

partex_engine::persist_struct!(Dest {
    rect,
    kind,
    zoom,
    struct_num,
    named
});

/// The state of PDF shipping (pdfTeX's globals of parts 32c and 32f),
/// with the fonts' and the encodings' tables apart: three fields.
#[derive(Clone, Debug, Default, PartialEq, Hash)]
pub(crate) struct Ship {
    /// The `SHIP` field.
    pub st: Val<ShipState>,
    /// (hashed apart: `PdfFonts`)
    pub fonts: PdfFonts,
    /// writeenc.c's `fe_tree` (the `ENCODINGS` field).
    pub encodings: VMap<Vec<u8>, GlyphNames>,
    /// The glyphs each ship used, by font, by the ship's number (with
    /// values: `Row::Glyphs`, DESIGN 7.17.3 item 5). A font's glyphs
    /// used are the union of these, made at the job's end; no ship reads
    /// another's, so a page whose glyphs changed makes only the job's end
    /// dirty.
    pub glyphs: Vec<Glyphs>,
    /// The glyphs the ship running has used so far (scratch).
    pub(crate) marking: alloc::collections::BTreeMap<i32, [u64; 4]>,
}

/// The glyphs a ship used: each font's, in font order.
pub(crate) type Glyphs = Arc<[(i32, [u64; 4])]>;

/// Each font's glyphs used: the union of the ships' `gl`, as the job's
/// end makes it, and as a rebuild makes it again to see whether the end
/// must run (DESIGN 4.3, "The job's end").
pub(crate) fn glyph_union<'a>(
    gl: impl IntoIterator<Item = &'a Glyphs>,
) -> alloc::collections::BTreeMap<i32, [u64; 4]> {
    let mut sets = alloc::collections::BTreeMap::<i32, [u64; 4]>::new();
    for (f, c) in gl.into_iter().flat_map(|g| g.iter()) {
        let s = sets.entry(*f).or_default();
        for (a, b) in s.iter_mut().zip(c) {
            *a |= b;
        }
    }
    sets
}

partex_engine::persist_struct!(Ship {
    st,
    fonts,
    encodings,
    glyphs,
    marking
});

impl core::ops::Deref for Ship {
    type Target = ShipState;
    #[inline]
    fn deref(&self) -> &ShipState {
        &self.st
    }
}

impl core::ops::DerefMut for Ship {
    #[inline]
    fn deref_mut(&mut self) -> &mut ShipState {
        &mut self.st
    }
}

/// The state of shipping, the tables apart (pdfTeX's globals of parts
/// 32c and 32f).
#[derive(Clone, Debug, Default, PartialEq, Hash)]
pub(crate) struct ShipState {
    /// `init_pdf_output`.
    pub init: bool,
    pub cur_h: Scaled,
    pub cur_v: Scaled,
    pub cur_s: i32,
    doing_leaders: bool,
    pdf_f: i32,
    pdf_h: Scaled,
    pdf_v: Scaled,
    tj_start_h: Scaled,
    delta_h: Scaled,
    pub origin_h: Scaled,
    pub origin_v: Scaled,
    doing_string: bool,
    doing_text: bool,
    /// `pdf_cur_Tm_a`.
    tm_a: i32,
    last_f: i32,
    last_fs: Scaled,
    min_bp_val: Scaled,
    /// `pdf_dummy_font` (0: not read).
    pub dummy_font: i32,
    /// `is_shipping_page`.
    pub shipping_page: bool,
    pub page_width: Scaled,
    pub page_height: Scaled,
    h_offset: Scaled,
    v_offset: Scaled,
    xform_width: Scaled,
    xform_height: Scaled,
    xform_depth: Scaled,
    /// `pdf_cur_form`.
    pub cur_form: i32,
    pub total_pages: i32,
    /// How many times `pdf_ship_out` ran (pages and forms): the next
    /// ship's glyph slot (`Row::Glyphs`).
    pub ships: u32,
    pub last_page: i32,
    pub last_pages: i32,
    last_stream: i32,
    page_group_val: i32,
    pub font_list: Vec<i32>,
    obj_list: Vec<i32>,
    xform_list: Vec<i32>,
    pub ximage_list: Vec<i32>,
    text_procset: bool,
    /// `pdf_image_procset`.
    image_procset: i32,
    annot_list: Vec<i32>,
    link_list: Vec<i32>,
    dest_list: Vec<i32>,
    link_stack: Vec<LinkLevel>,
    pub faked_space: bool,
    /// Not `gen_running_link`.
    no_running_link: bool,
    /// A content stream is open: display items are recorded
    /// (`draw.rs`).
    pub recording: bool,
    /// The display items recorded and not encoded yet.
    pub drawn: Vec<super::draw::Drawn>,
}

super::val::record_by_hash!(ShipState);

impl Ship {
    /// The per-page scratch that is dead between ships, set to its
    /// initial value, or to junk if `poison` (a machine's clean points,
    /// DESIGN §7.16.1):
    /// - the page's annotation, link and destination lists: emptied as a
    ///   page ship begins (pdfTeX §752, `pdf_ship_box_out`), read only at
    ///   that page's end (`write_page_object`, `write_page_marks`);
    /// - `pdf_v`: set by `pdf_set_origin` when text begins (pdfTeX §727–
    ///   §735: `pdf_begin_text`), read only inside text
    ///   (`pdf_begin_string`, `pdf_set_text_pos`); a ship ends its text
    ///   (`Draw::EndText`), so no ship begins inside one.
    pub(crate) fn canonicalize(&mut self, poison: bool) {
        self.annot_list.clear();
        self.link_list.clear();
        self.dest_list.clear();
        self.pdf_v = 0;
        if poison {
            self.annot_list.push(0x7fff_0001);
            self.link_list.push(0x7fff_0002);
            self.dest_list.push(0x7fff_0003);
            self.pdf_v = 0x0123_4567;
        }
    }

    /// The fields that differ from `other`'s, and for fonts, which fonts'
    /// used characters differ (`PARTEX_WATCH_DEBUG`).
    pub(crate) fn differences(&self, other: &Self) -> Vec<alloc::string::String> {
        let mut out = Vec::new();
        macro_rules! cmp {
            ($($f:ident),*) => {$(
                if self.$f != other.$f {
                    out.push(alloc::string::String::from(stringify!($f)));
                }
            )*};
        }
        cmp!(
            init,
            cur_h,
            cur_v,
            cur_s,
            doing_leaders,
            pdf_f,
            pdf_h,
            pdf_v,
            tj_start_h,
            delta_h,
            origin_h,
            origin_v,
            doing_string,
            doing_text,
            tm_a,
            last_f,
            last_fs,
            min_bp_val,
            dummy_font,
            shipping_page,
            page_width,
            page_height,
            h_offset,
            v_offset,
            xform_width,
            xform_height,
            xform_depth,
            cur_form,
            total_pages,
            last_page,
            last_pages,
            last_stream,
            page_group_val,
            font_list,
            obj_list,
            xform_list,
            ximage_list,
            text_procset,
            image_procset,
            annot_list,
            link_list,
            dest_list,
            link_stack,
            faked_space,
            no_running_link,
            encodings
        );
        for (f, (a, b)) in self.fonts.iter().zip(&other.fonts).enumerate() {
            if a != b {
                let what = if a.chars == b.chars {
                    "fields"
                } else {
                    "chars"
                };
                out.push(alloc::format!("fonts[{f}] {what}"));
            }
        }
        if self.fonts.len() != other.fonts.len() {
            out.push(alloc::string::String::from("fonts: count"));
        }
        out
    }
}

partex_engine::persist_struct!(ShipState {
    init,
    cur_h,
    cur_v,
    cur_s,
    doing_leaders,
    pdf_f,
    pdf_h,
    pdf_v,
    tj_start_h,
    delta_h,
    origin_h,
    origin_v,
    doing_string,
    doing_text,
    tm_a,
    last_f,
    last_fs,
    min_bp_val,
    dummy_font,
    shipping_page,
    page_width,
    page_height,
    h_offset,
    v_offset,
    xform_width,
    xform_height,
    xform_depth,
    cur_form,
    total_pages,
    ships,
    last_page,
    last_pages,
    last_stream,
    page_group_val,
    font_list,
    obj_list,
    xform_list,
    ximage_list,
    text_procset,
    image_procset,
    annot_list,
    link_list,
    dest_list,
    link_stack,
    faked_space,
    no_running_link,
    recording,
    drawn
});

impl<H: Host, T: Tracker> Tex<H, T> {
    /// The PDF state of font `f`.
    /// Mark glyph `c` of font `f` used, and note it among the glyphs this
    /// stretch of the job used (a machine's `Glyphs` cell; a session's
    /// `Interval.chars`, [`Tex::take_chars_shipped`]).
    pub(crate) fn mark_glyph(&mut self, f: i32, c: u8) {
        self.pdf_font(f).mark(c);
        if T::VALUES {
            self.pdf.ship.marking.entry(f).or_default()[usize::from(c >> 6)] |= 1 << (c & 63);
        }
        let i = crate::fonts::fx(f);
        if self.glyphs_used.len() <= i {
            self.glyphs_used.resize(i + 1, [0; 4]);
        }
        self.glyphs_used[i][usize::from(c >> 6)] |= 1 << (c & 63);
    }

    /// The glyphs of each PDF font shipped since the last call, by font,
    /// cleared: what a stretch of the job used (read-set cutoff keeps a
    /// later checkpoint's font subsets right, `chars_differ`).
    pub fn take_chars_shipped(&mut self) -> alloc::vec::Vec<(usize, [u64; 4])> {
        let v = core::mem::take(&mut self.glyphs_used);
        v.into_iter()
            .enumerate()
            .filter(|(_, c)| *c != [0; 4])
            .collect()
    }

    pub(crate) fn pdf_font(&mut self, f: i32) -> &mut PdfFont {
        use super::val::{bit, field::PDF_FONTS};
        if T::VALUES {
            // (the `PDF_FONTS` field: read, and written by the scope of
            // the writer that changes it; outside one (font expansion
            // asks a font's type), only read)
            self.writer_read(PDF_FONTS);
        } else {
            // (what the PDF writer keeps of a font is part of its state, a
            // machine's `MCell::Font`: read, and perhaps changed)
            self.tracker.read(crate::track::Cell::Font(f));
            self.tracker.write(crate::track::Cell::Font(f));
        }
        let i = crate::fonts::fx(f);
        if self.pdf.ship.fonts.len() <= i {
            // (slots made: a write of the table)
            self.writer_scope(0, bit(PDF_FONTS), |t| {
                t.pdf.ship.fonts.resize(i + 1, PdfFont::default());
            });
        }
        &mut self.pdf.ship.fonts[i]
    }

    /// The name of font `f`.
    pub(crate) fn font_name_bytes(&self, f: i32) -> Vec<u8> {
        let s = crate::input::ux(self.fonts.name[crate::fonts::fx(f)]);
        self.str_pool[self.str_start[s]..self.str_start[s + 1]].to_vec()
    }

    /// mapfile.c's `hasfmentry`: the map entry of font `f`, looked up
    /// (reading the default map file) the first time.
    pub(crate) fn fm_entry(&mut self, f: i32) -> Option<Arc<MapEntry>> {
        if let Some(m) = &self.pdf_font(f).map {
            return m.clone();
        }
        // an auto-expanded font is its base font's (`pdf_init_font`)
        let x = self.fonts.expand[crate::fonts::fx(f)];
        if x.auto && x.blink != NULL_FONT {
            let m = self.fm_entry(x.blink);
            self.pdf_font(f).map = Some(m.clone());
            return m;
        }
        if !self.fontmap.read {
            self.read_default_map();
        }
        let name = self.font_name_bytes(f);
        let m = self.fontmap.lookup(&name);
        if m.is_some() {
            self.fonts_mapped.insert(name);
        }
        self.pdf_font(f).map = Some(m.clone());
        m
    }

    /// mapfile.c's `isscalable`.
    pub(crate) fn is_scalable(&mut self, f: i32) -> bool {
        self.fm_entry(f).is_some_and(|m| !m.is(F_PK))
    }

    /// mapfile.c's `hasspacechar`.
    fn has_space_char(&mut self, f: i32) -> Result<bool, Jump> {
        if !self.is_scalable(f) {
            return Ok(false);
        }
        let Some(enc) = self.fm_entry(f).and_then(|m| m.encname.clone()) else {
            return Ok(false);
        };
        let names = self.get_fe_entry(&enc)?;
        Ok(names[32] == b"space")
    }

    /// utils.c's `pdftex_fail`: a fatal error; the job ends at once.
    pub(crate) fn pdftex_fail<R>(&mut self, file: Option<&[u8]>, msg: &[u8]) -> Result<R, Jump> {
        self.print_ln();
        self.print_str(b"!pdfTeX error: ");
        let name = self.params.invocation_name.clone();
        self.print_str(&name);
        if let Some(f) = file {
            self.print_str(b" (file ");
            self.print_str(f);
            self.print_str(b")");
        }
        self.print_str(b": ");
        self.print_str(msg);
        self.print_ln();
        self.print_str(b" ==> Fatal error occurred, no output PDF file produced!");
        self.print_ln();
        self.set_history(crate::error::FATAL_ERROR_STOP);
        Err(Jump::FinalEnd)
    }

    // ---- pdfTeX §727–§735: page description ----

    /// `pdf_set_origin`.
    fn pdf_set_origin(&mut self, h: Scaled, v: Scaled) -> Result<(), Jump> {
        let s = &self.pdf.ship;
        if (h - s.origin_h).abs() >= s.min_bp_val || (v - s.origin_v).abs() >= s.min_bp_val {
            self.pdf.out.print(b"1 0 0 1 ");
            self.pdf_print_bp(h - self.pdf.ship.origin_h)?;
            self.pdf.ship.origin_h += self.pdf.out.scaled_out;
            self.pdf.out.out(b' ');
            self.pdf_print_bp(self.pdf.ship.origin_v - v)?;
            self.pdf.ship.origin_v -= self.pdf.out.scaled_out;
            self.pdf.out.print_ln(b" cm");
        }
        let s = &mut *self.pdf.ship;
        s.pdf_h = s.origin_h;
        s.tj_start_h = s.pdf_h;
        s.pdf_v = s.origin_v;
        Ok(())
    }

    /// `pdf_set_origin_temp`.
    fn pdf_set_origin_temp(&mut self, h: Scaled, v: Scaled) -> Result<(), Jump> {
        let s = &self.pdf.ship;
        if (h - s.origin_h).abs() >= s.min_bp_val || (v - s.origin_v).abs() >= s.min_bp_val {
            self.pdf.out.print(b"1 0 0 1 ");
            self.pdf_print_bp(h - self.pdf.ship.origin_h)?;
            self.pdf.out.out(b' ');
            self.pdf_print_bp(self.pdf.ship.origin_v - v)?;
            self.pdf.out.print_ln(b" cm");
        }
        Ok(())
    }

    /// `pdf_end_string`.
    fn pdf_end_string(&mut self) {
        if self.pdf.ship.doing_string {
            self.pdf.out.print(b")]TJ");
            self.pdf.ship.doing_string = false;
        }
    }

    /// `pdf_end_string_nl`.
    fn pdf_end_string_nl(&mut self) {
        if self.pdf.ship.doing_string {
            self.pdf.out.print_ln(b")]TJ");
            self.pdf.ship.doing_string = false;
        }
    }

    /// `get_font_auto_expand_ratio`: how much `f` is expanded, if
    /// auto-expanded.
    fn auto_expand_ratio(&self, f: i32) -> i32 {
        let x = self.fonts.expand[crate::fonts::fx(f)];
        if x.auto { x.ratio } else { 0 }
    }

    /// `pdf_set_text_pos`: a text matrix for an auto-expanded font (or
    /// back from one), else a move.
    fn pdf_set_text_pos(&mut self, v: i32, v_out: Scaled, f: i32) -> Result<(), Jump> {
        self.pdf.out.out(b' ');
        let tm_a = self.auto_expand_ratio(f);
        if tm_a != 0 || self.pdf.ship.tm_a != 0 {
            self.pdf.out.print_real(1000 + tm_a, 3);
            self.pdf.out.print(b" 0 0 1 ");
            self.pdf_print_bp(self.pdf.ship.cur_h - self.pdf.ship.origin_h)?;
            self.pdf.ship.pdf_h = self.pdf.ship.origin_h + self.pdf.out.scaled_out;
            self.pdf.out.out(b' ');
            self.pdf_print_bp(self.pdf.ship.origin_v - self.pdf.ship.cur_v)?;
            self.pdf.ship.pdf_v = self.pdf.ship.origin_v - self.pdf.out.scaled_out;
            self.pdf.out.print(b" Tm");
            self.pdf.ship.tm_a = tm_a;
        } else {
            self.pdf_print_bp(self.pdf.ship.cur_h - self.pdf.ship.tj_start_h)?;
            self.pdf.ship.pdf_h = self.pdf.ship.tj_start_h + self.pdf.out.scaled_out;
            self.pdf.out.out(b' ');
            let dd = self.pdf.out.fixed_decimal_digits;
            self.pdf.out.print_real(v, dd);
            self.pdf.ship.pdf_v -= v_out;
            self.pdf.out.print(b" Td");
        }
        let s = &mut *self.pdf.ship;
        s.tj_start_h = s.pdf_h;
        s.delta_h = 0;
        Ok(())
    }

    /// `pdf_use_font`.
    fn pdf_use_font(&mut self, f: i32, num: i32) -> Result<(), Jump> {
        let size = self.fonts.get(f).size;
        let (_, out) = self.divide_scaled(size, ONE_HUNDRED_BP, 6)?;
        let pf = self.pdf_font(f);
        pf.size = out;
        pf.used = true;
        pf.num = num;
        if self.int_par(PDF_MOVE_CHARS_CODE) > 0 {
            self.pdf_warning(b"", b"Primitive \\pdfmovechars is obsolete.", true, true);
            self.set_int_par(PDF_MOVE_CHARS_CODE, 0);
        }
        Ok(())
    }

    /// `pdf_init_font`.
    pub(crate) fn pdf_init_font(&mut self, f: i32) -> Result<(), Jump> {
        // if `f` is auto-expanded, its base font first
        let x = self.fonts.expand[crate::fonts::fx(f)];
        let expanded_from = (x.auto && x.blink != NULL_FONT).then_some(x.blink);
        if let Some(b) = expanded_from {
            if !self.is_scalable(b) {
                return self.pdf_error(
                    b"font expansion",
                    b"auto expansion is only possible with scalable fonts",
                );
            }
            if !self.pdf_font(b).used {
                self.pdf_init_font(b)?;
            }
        }
        if self.is_scalable(f) {
            let map = self.fm_entry(f);
            let name = self.font_name_bytes(f);
            let base_name = expanded_from.map(|b| self.font_name_bytes(b));
            let mut i = self.pdf.objs.head[super::objtab::OBJ_TYPE_FONT];
            while i != 0 {
                let k = self.pdf.objs.get(i).info.num();
                let same_map = match (&map, &self.fm_entry(k)) {
                    (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                    _ => false,
                };
                let k_name = self.font_name_bytes(k);
                if same_map
                    && self.is_scalable(k)
                    && (k_name == name || base_name.as_ref() == Some(&k_name))
                {
                    let nk = self.pdf_font(k).num;
                    return self.pdf_use_font(f, if nk < 0 { nk } else { -k });
                }
                i = self.pdf.objs.get(i).link;
            }
        }
        let k = self.pdf_create_obj(super::objtab::OBJ_TYPE_FONT, Id::Num(f))?;
        let space =
            self.has_space_char(f)? && self.fonts.get(f).glyph(32).map_or(0, |g| g.width) > ONE_BP;
        self.pdf_font(f).has_space = space;
        self.pdf_use_font(f, k)
    }

    /// `set_ff`: the font whose object font `f` uses.
    pub(crate) fn pdf_ff(&mut self, f: i32) -> i32 {
        let n = self.pdf_font(f).num;
        if n < 0 { -n } else { f }
    }

    /// Font `f`'s number in a `/F` name (tex.web's, `FontArrays::number`;
    /// with fonts as cells a relocation, the link's: the region then does
    /// not depend on how many fonts were loaded before `f`).
    fn pdf_print_font_number(&mut self, f: i32) {
        if self.pdf.out.font_refs && self.pdf.out.virt {
            self.pdf.out.fontref(f);
        } else {
            let n = self.fonts.number(f);
            self.pdf.out.print_int(i64::from(n));
        }
    }

    /// `pdf_set_font`.
    fn pdf_set_font(&mut self, f: i32) -> Result<(), Jump> {
        self.pdf.ship.pdf_f = f;
        if !self.pdf_font(f).used {
            self.pdf_init_font(f)?;
        }
        let k = self.pdf_ff(f);
        let list = self.pdf.ship.font_list.clone();
        if !list.iter().any(|&g| self.pdf_ff(g) == k) {
            self.pdf.ship.font_list.push(f);
        }
        let size = self.fonts.get(f).size;
        if k == self.pdf.ship.last_f && size == self.pdf.ship.last_fs {
            return Ok(());
        }
        self.pdf.out.print(b"/F");
        self.pdf_print_font_number(k);
        self.pdf_print_resname_prefix();
        self.pdf.out.out(b' ');
        let (q, _) = self.divide_scaled(size, ONE_HUNDRED_BP, 6)?;
        self.pdf.out.print_real(q, 4);
        self.pdf.out.print(b" Tf");
        self.pdf.ship.last_f = k;
        self.pdf.ship.last_fs = size;
        Ok(())
    }

    /// `pdf_print_resname_prefix` (`\pdfuniqueresname` is not supported).
    #[expect(clippy::unused_self, reason = "pdfTeX's hook")]
    pub(crate) fn pdf_print_resname_prefix(&mut self) {}

    /// `pdf_begin_text`.
    fn pdf_begin_text(&mut self) -> Result<(), Jump> {
        let h = self.pdf.ship.page_height;
        self.pdf_set_origin(0, h)?;
        self.pdf.out.print_ln(b"BT");
        let s = &mut *self.pdf.ship;
        s.doing_text = true;
        s.pdf_f = NULL_FONT;
        s.last_f = NULL_FONT;
        s.last_fs = 0;
        s.doing_string = false;
        s.tm_a = 0;
        Ok(())
    }

    /// `pdf_read_dummy_font`.
    fn pdf_read_dummy_font(&mut self) -> Result<(), Jump> {
        if self.pdf.ship.dummy_font == NULL_FONT {
            let name = (*self.pdf.space_font_name)
                .clone()
                .unwrap_or_else(|| b"pdftexspace".to_vec());
            let nom = self.make_tex_string(&name)?;
            let aire = self.make_tex_string(b"")?;
            self.pdf.ship.dummy_font = self.read_font_info(NULL_CS, nom, aire, -1000)?;
            self.process_map_item(b"=pdftexspace PdfTeX-Space <pdftexspace.pfb", false);
            let d = self.pdf.ship.dummy_font;
            self.mark_glyph(d, 32);
        }
        Ok(())
    }

    /// `pdf_insert_interword_space`.
    fn pdf_insert_interword_space(&mut self) -> Result<(), Jump> {
        self.pdf_read_dummy_font()?;
        let d = self.pdf.ship.dummy_font;
        self.pdf_set_font(d)?;
        self.pdf.out.print(b"( )Tj");
        Ok(())
    }

    /// `pdf_begin_string`.
    pub(crate) fn pdf_begin_string(&mut self, f: i32) -> Result<(), Jump> {
        let mut must_end_string = false;
        let mut must_insert_space = false;
        let mut must_set_text_pos = false;
        if !self.pdf.ship.doing_text {
            self.pdf_begin_text()?;
            must_set_text_pos = true;
        }
        let faked = self.pdf.ship.faked_space;
        let pdf_f = self.pdf.ship.pdf_f;
        let switch = if faked {
            !(self.pdf_font(f).used && self.pdf_font(pdf_f).used)
        } else {
            pdf_f != f
        };
        if switch {
            self.pdf_end_string();
            self.pdf_set_font(f)?;
        }
        let size = self.pdf_font(f).size;
        let s0 = &self.pdf.ship;
        let (mut s, mut s_out) = if s0.tm_a == 0 {
            self.divide_scaled(s0.cur_h - (s0.tj_start_h + s0.delta_h), size, 3)?
        } else {
            let tm_a = s0.tm_a;
            let d = round_xn_over_d(s0.cur_h - (s0.tj_start_h + s0.delta_h), 1000, 1000 + tm_a);
            let (s, _) = self.divide_scaled(d, size, 3)?;
            let mut out = 0;
            if s.abs() < 0o100000 {
                out = round_xn_over_d(round_xn_over_d(size, s.abs(), 1000), 1000 + tm_a, 1000);
                if s < 0 {
                    out = -out;
                }
            }
            (s, out)
        };
        let (cur_v, pdf_v) = (self.pdf.ship.cur_v, self.pdf.ship.pdf_v);
        let (v, v_out) = if (cur_v - pdf_v).abs() >= self.pdf.ship.min_bp_val {
            let dd = self.pdf.out.fixed_decimal_digits;
            self.divide_scaled(pdf_v - cur_v, ONE_HUNDRED_BP, dd + 2)?
        } else {
            (0, 0)
        };
        if !must_set_text_pos {
            let a = self.auto_expand_ratio(f);
            must_set_text_pos = v != 0
                || s.abs() >= 0o100000
                || a != self.pdf.ship.tm_a
                || a != self.auto_expand_ratio(self.pdf.ship.pdf_f);
        }
        if must_set_text_pos {
            must_end_string = true;
        }
        let font = self.fonts.get(f);
        let (space, shrink) = (font.param(2), font.param(4));
        let looks_like_space = space > ONE_BP && s_out > space - shrink - ONE_BP / 10 && v == 0;
        if faked && looks_like_space {
            must_insert_space = true;
        }
        if must_insert_space {
            if self.pdf_font(f).has_space && self.pdf.ship.doing_string {
                self.pdf.out.out(b' ');
                let (ws, wout) = self.adv_char_width(f, 32)?;
                s -= ws;
                s_out -= wout;
                self.mark_glyph(f, 32);
            } else {
                must_end_string = true;
            }
        }
        if must_end_string {
            self.pdf_end_string();
            if must_insert_space && !self.pdf_font(f).has_space {
                self.pdf_insert_interword_space()?;
            }
            self.pdf_set_font(f)?;
            self.pdf_set_text_pos(v, v_out, f)?;
            s = 0;
        }
        if faked && self.pdf.ship.pdf_f != f {
            self.pdf_end_string();
            self.pdf_set_font(f)?;
        }
        if !self.pdf.ship.doing_string {
            self.pdf.out.print(b" [");
            if s == 0 {
                self.pdf.out.out(b'(');
            }
        }
        if s != 0 {
            if self.pdf.ship.doing_string {
                self.pdf.out.out(b')');
            }
            self.pdf.out.print_int(i64::from(-s));
            self.pdf.out.out(b'(');
            self.pdf.ship.delta_h += s_out;
        }
        self.pdf.ship.doing_string = true;
        Ok(())
    }

    /// `adv_char_width`: advance `pdf_delta_h` by the width of `c` in the
    /// raster of the `/Widths`; returns (`adv_char_width_s`,
    /// `adv_char_width_s_out`).
    pub(crate) fn adv_char_width(&mut self, f: i32, c: u8) -> Result<(i32, Scaled), Jump> {
        let w = self.fonts.get(f).width(c);
        if !self.is_scalable(f) {
            return self.pdf_error(b"font", b"PK fonts are not implemented in partex yet");
        }
        let size = self.pdf_font(f).size;
        let tm_a = self.pdf.ship.tm_a;
        let (s, s_out) = if tm_a == 0 {
            self.divide_scaled(w, size, 4)?
        } else {
            let (s, _) = self.divide_scaled(round_xn_over_d(w, 1000, 1000 + tm_a), size, 4)?;
            let mut out = round_xn_over_d(round_xn_over_d(size, s.abs(), 10000), 1000 + tm_a, 1000);
            if s < 0 {
                out = -out;
            }
            (s, out)
        };
        self.pdf.ship.delta_h += s_out;
        // `round(s/10)`, halves away from zero
        let r = if s >= 0 { (s + 5) / 10 } else { (s - 5) / 10 };
        Ok((r, s_out))
    }

    /// `pdf_print_char`.
    pub(crate) fn pdf_print_char(&mut self, f: i32, c: u8) {
        self.mark_glyph(f, c);
        if c <= 32 || c == 92 || c == 40 || c == 41 || c > 127 {
            self.pdf.out.out(92);
            self.pdf.out.print_octal(c);
        } else {
            self.pdf.out.out(c);
        }
    }

    /// `pdf_insert_fake_space`.
    pub(crate) fn pdf_insert_fake_space(&mut self) -> Result<(), Jump> {
        let saved = self.pdf.ship.faked_space;
        self.pdf.ship.faked_space = false;
        self.pdf_read_dummy_font()?;
        let d = self.pdf.ship.dummy_font;
        self.pdf_begin_string(d)?;
        self.pdf.out.print(b" ");
        self.adv_char_width(d, 32)?;
        self.pdf_end_string_nl();
        self.pdf.ship.faked_space = saved;
        Ok(())
    }

    /// `pdf_end_text`.
    pub(crate) fn pdf_end_text(&mut self) {
        if self.pdf.ship.doing_text {
            self.pdf_end_string_nl();
            self.pdf.out.print_ln(b"ET");
            self.pdf.ship.doing_text = false;
        }
    }

    /// `pdf_set_rule` (pdfTeX §691).
    pub(crate) fn pdf_set_rule(
        &mut self,
        x: Scaled,
        y: Scaled,
        w: Scaled,
        h: Scaled,
    ) -> Result<(), Jump> {
        self.pdf_end_text();
        self.pdf.out.print_ln(b"q");
        if h <= ONE_BP {
            // pdfTeX §691: real division, then truncate the whole coordinate.
            let origin = (2 * i64::from(y) - i64::from(h) - 1) / 2;
            let Ok(origin) = Scaled::try_from(origin) else {
                return self.pdf_error(b"rule", b"position out of range");
            };
            self.pdf_set_origin_temp(x, origin)?;
            self.pdf.out.print(b"[]0 d 0 J ");
            self.pdf_print_bp(h)?;
            self.pdf.out.print(b" w 0 0 m ");
            self.pdf_print_bp(w)?;
            self.pdf.out.print_ln(b" 0 l S");
        } else if w <= ONE_BP {
            let origin = (2 * i64::from(x) + i64::from(w) + 1) / 2;
            let Ok(origin) = Scaled::try_from(origin) else {
                return self.pdf_error(b"rule", b"position out of range");
            };
            self.pdf_set_origin_temp(origin, y)?;
            self.pdf.out.print(b"[]0 d 0 J ");
            self.pdf_print_bp(w)?;
            self.pdf.out.print(b" w 0 0 m 0 ");
            self.pdf_print_bp(h)?;
            self.pdf.out.print_ln(b" l S");
        } else {
            self.pdf_set_origin_temp(x, y)?;
            self.pdf.out.print(b"0 0 ");
            self.pdf_print_bp(w)?;
            self.pdf.out.out(b' ');
            self.pdf_print_bp(h)?;
            self.pdf.out.print_ln(b" re f");
        }
        self.pdf.out.print_ln(b"Q");
        Ok(())
    }

    /// `pdf_rectangle`.
    fn pdf_rectangle(&mut self, r: Rect) -> Result<(), Jump> {
        self.prepare_mag()?;
        self.pdf.out.print(b"/Rect [");
        self.pdf_print_rect_spec(r)?;
        self.pdf.out.print_ln(b"]");
        Ok(())
    }

    /// `pdf_print_rect_spec`.
    fn pdf_print_rect_spec(&mut self, r: Rect) -> Result<(), Jump> {
        let (oh, ov) = (self.pdf.ship.origin_h, self.pdf.ship.origin_v);
        self.pdf_print_mag_bp(r.left - oh)?;
        self.pdf.out.out(b' ');
        self.pdf_print_mag_bp(ov - r.bottom)?;
        self.pdf.out.out(b' ');
        self.pdf_print_mag_bp(r.right - oh)?;
        self.pdf.out.out(b' ');
        self.pdf_print_mag_bp(ov - r.top)
    }

    /// `literal`.
    pub(crate) fn literal(&mut self, s: &[u8], mode: i32, warn: bool) -> Result<(), Jump> {
        let mut j = 0;
        let mut mode = mode;
        if mode == SCAN_SPECIAL {
            if !(s.starts_with(b"PDF:") || s.starts_with(b"pdf:")) {
                if warn && !(s.starts_with(b"SRC:") || s.starts_with(b"src:") || s.is_empty()) {
                    self.print_nl(b"Non-PDF special ignored!");
                    self.print_nl(b"<special> ");
                    // `slow_print_substr(s, 64)`
                    for &c in s.iter().take(65) {
                        self.print(i32::from(c));
                    }
                    if s.len() > 65 {
                        self.print_str(b"...");
                    }
                    self.print_ln();
                }
                return Ok(());
            }
            j = 4;
            if s[4..].starts_with(b"direct:") {
                j += 7;
                mode = DIRECT_ALWAYS;
            } else if s[4..].starts_with(b"page:") {
                j += 5;
                mode = DIRECT_PAGE;
            } else {
                mode = SET_ORIGIN;
            }
        }
        if !matches!(mode, SET_ORIGIN | DIRECT_PAGE | DIRECT_ALWAYS) {
            return self.confusion(b"literal1");
        }
        self.draw(Draw::Literal {
            text: s[j..].to_vec(),
            mode,
        })
    }

    /// The encoder's half of `literal`: `text` in resolved `mode`.
    pub(crate) fn emit_literal(&mut self, text: &[u8], mode: i32) -> Result<(), Jump> {
        match mode {
            SET_ORIGIN => {
                self.pdf_end_text();
                let (h, v) = (self.pdf.ship.cur_h, self.pdf.ship.cur_v);
                self.pdf_set_origin(h, v)?;
            }
            DIRECT_PAGE => self.pdf_end_text(),
            _ => self.pdf_end_string_nl(),
        }
        self.pdf.out.print(text);
        self.pdf.out.out(b'\n');
        Ok(())
    }

    /// Expand a late text (`\pdfliteral shipout`, `\special shipout`).
    fn late_text(&mut self, tokens: &partex_engine::node::Tokens) -> Result<Vec<u8>, Jump> {
        self.expand_write_text(tokens)?;
        let expanded = self.take_def();
        Ok(self.tokens_string(&expanded))
    }

    /// `pdf_out_colorstack`.
    fn pdf_out_colorstack(
        &mut self,
        stack: i32,
        cmd: u8,
        data: Option<&partex_engine::node::Tokens>,
    ) -> Result<(), Jump> {
        let used = i32::try_from(self.pdf.stacks.used()).unwrap_or(i32::MAX);
        if stack >= used {
            self.print_nl(b"");
            self.print_str(b"Color stack ");
            self.print_int(stack);
            self.print_str(b" is not initialized for use!");
            self.print_nl(b"");
            return Ok(());
        }
        let (mode, s) = match cmd {
            0 | 1 => {
                let s = data.map(|d| self.tokens_string(d)).unwrap_or_default();
                let mode = if cmd == 0 {
                    self.pdf.stacks.set(stack, &s)
                } else {
                    self.pdf.stacks.push(stack, &s)
                };
                (mode, s)
            }
            2 => {
                let (mode, s) = self.pdf.stacks.pop(stack);
                if s.is_none() {
                    let kind = if self.pdf.stacks.page_mode {
                        "page"
                    } else {
                        "form"
                    };
                    let m = alloc::format!("pop empty color {kind} stack {stack}");
                    self.pdftex_warn(m.as_bytes());
                }
                (mode, s.unwrap_or_default())
            }
            3 => self.pdf.stacks.current(stack),
            _ => return self.confusion(b"pdfcolorstack"),
        };
        if !s.is_empty() {
            self.literal(&s, mode, false)?;
        }
        Ok(())
    }

    /// `pdf_out_colorstack_startpage`.
    fn pdf_out_colorstack_startpage(&mut self) -> Result<(), Jump> {
        let max = i32::try_from(self.pdf.stacks.used()).unwrap_or(0);
        for i in 0..max {
            if self.pdf.stacks.skip_page_start(i) == 0 {
                let (mode, s) = self.pdf.stacks.current(i);
                if !s.is_empty() {
                    self.literal(&s, mode, false)?;
                }
            }
        }
        Ok(())
    }

    /// `set_rect_dimens`.
    fn set_rect_dimens(
        &mut self,
        parent: &BoxNode,
        x: Scaled,
        y: Scaled,
        d: Dims,
        margin: Scaled,
    ) -> Rect {
        let s = &self.pdf.ship;
        let mut r = Rect {
            left: s.cur_h,
            right: if d.width == RUNNING {
                x + parent.width
            } else {
                s.cur_h + d.width
            },
            top: if d.height == RUNNING {
                y - parent.height
            } else {
                s.cur_v - d.height
            },
            bottom: if d.depth == RUNNING {
                y + parent.depth
            } else {
                s.cur_v + d.depth
            },
        };
        if s.shipping_page && self.pdf.stacks.matrix_used() {
            let ph = s.page_height;
            let [llx, lly, urx, ury] =
                self.pdf
                    .stacks
                    .transform_rect(r.left, ph - r.bottom, r.right, ph - r.top);
            r = Rect {
                left: llx,
                bottom: ph - lly,
                right: urx,
                top: ph - ury,
            };
        }
        r.left -= margin;
        r.top -= margin;
        r.right += margin;
        r.bottom += margin;
        r
    }

    /// `get_obj`: the object of type `t` for identifier `id`, created if
    /// new.
    pub(crate) fn get_obj(&mut self, t: usize, id: Id) -> Result<i32, Jump> {
        let r = self.pdf.objs.find(t, &id);
        if r != 0 {
            return Ok(r);
        }
        self.pdf_create_obj(t, id)
    }

    /// `do_annot`.
    fn do_annot(
        &mut self,
        dims: Dims,
        data: &partex_engine::node::Tokens,
        objnum: i32,
        parent: &BoxNode,
        x: Scaled,
        y: Scaled,
    ) -> Result<(), Jump> {
        if !self.pdf.ship.shipping_page {
            return self.pdf_error(b"ext4", b"annotations cannot be inside an XForm");
        }
        if self.pdf.ship.doing_leaders {
            return Ok(());
        }
        let objnum = if self.pdf.objs.is_scheduled(objnum) {
            self.pdf_new_objnum()?
        } else {
            objnum
        };
        let rect = self.set_rect_dimens(parent, x, y, dims, 0);
        self.pdf.objs.get_mut(objnum).aux = Aux::Mark(Box::new(Mark {
            rect,
            data: Some(data.clone()),
            action: None,
        }));
        self.pdf.ship.annot_list.push(objnum);
        self.pdf.objs.set_scheduled(objnum);
        Ok(())
    }

    /// `do_link`.
    #[expect(clippy::too_many_arguments, reason = "the node's fields")]
    fn do_link(
        &mut self,
        dims: Dims,
        attr: Option<&partex_engine::node::Tokens>,
        action: &Arc<Action>,
        objnum: i32,
        parent: &BoxNode,
        x: Scaled,
        y: Scaled,
    ) -> Result<(), Jump> {
        if !self.pdf.ship.shipping_page {
            return self.pdf_error(b"ext4", b"link annotations cannot be inside an XForm");
        }
        let objnum = if self.pdf.objs.is_scheduled(objnum) {
            self.pdf_new_objnum()?
        } else {
            objnum
        };
        if self.pdf.ship.link_stack.len() >= PDF_MAX_LINK_LEVEL {
            self.overflow(b"pdf link stack size", 10)?;
        }
        let level = self.pdf.ship.cur_s;
        self.pdf.ship.link_stack.push(LinkLevel {
            level,
            dims,
            attr: attr.cloned(),
            action: action.clone(),
            objnum,
        });
        let margin = self.dimen_par(PDF_LINK_MARGIN_CODE);
        let rect = self.set_rect_dimens(parent, x, y, dims, margin);
        self.pdf.objs.get_mut(objnum).aux = Aux::Mark(Box::new(Mark {
            rect,
            data: attr.cloned(),
            action: Some(action.clone()),
        }));
        self.pdf.ship.link_list.push(objnum);
        self.pdf.objs.set_scheduled(objnum);
        Ok(())
    }

    /// `append_link`: continue running link `i` in another box.
    fn append_link(
        &mut self,
        parent: &BoxNode,
        x: Scaled,
        y: Scaled,
        i: usize,
    ) -> Result<(), Jump> {
        let l = self.pdf.ship.link_stack[i].clone();
        let margin = self.dimen_par(PDF_LINK_MARGIN_CODE);
        let rect = self.set_rect_dimens(parent, x, y, l.dims, margin);
        let k = self.pdf_new_objnum()?;
        self.pdf.ship.link_stack[i].objnum = k;
        self.pdf.objs.get_mut(k).aux = Aux::Mark(Box::new(Mark {
            rect,
            data: l.attr,
            action: Some(l.action),
        }));
        self.pdf.ship.link_list.push(k);
        Ok(())
    }

    /// `end_link`.
    fn end_link(&mut self) -> Result<(), Jump> {
        let Some(top) = self.pdf.ship.link_stack.last().cloned() else {
            return self.pdf_error(
                b"ext4",
                b"pdf_link_stack empty, \\pdfendlink used without \\pdfstartlink?",
            );
        };
        if top.level != self.pdf.ship.cur_s {
            self.pdf_warning(
                b"",
                b"\\pdfendlink ended up in different nesting level than \\pdfstartlink",
                true,
                true,
            );
        }
        if top.dims.width == RUNNING {
            let margin = self.dimen_par(PDF_LINK_MARGIN_CODE);
            let s = &self.pdf.ship;
            let (cur_h, ph, page) = (s.cur_h, s.page_height, s.shipping_page);
            let new = if page && self.pdf.stacks.matrix_used() {
                let [llx, lly, urx, ury] = self.pdf.stacks.recalculate(cur_h + margin);
                Some(Rect {
                    left: llx - margin,
                    top: ph - ury - margin,
                    right: urx + margin,
                    bottom: ph - lly + margin,
                })
            } else {
                None
            };
            if let Aux::Mark(m) = &mut self.pdf.objs.get_mut(top.objnum).aux {
                match new {
                    Some(r) => m.rect = r,
                    None => m.rect.right = cur_h + margin,
                }
            }
        }
        self.pdf.ship.link_stack.pop();
        Ok(())
    }

    /// `do_dest`.
    #[expect(clippy::too_many_arguments, reason = "the node's fields")]
    fn do_dest(
        &mut self,
        dims: Dims,
        struct_num: Option<i32>,
        id: &PdfId,
        kind: u8,
        zoom: Option<i32>,
        parent: &BoxNode,
        x: Scaled,
        y: Scaled,
    ) -> Result<(), Jump> {
        if !self.pdf.ship.shipping_page {
            return self.pdf_error(b"ext4", b"destinations cannot be inside an XForm");
        }
        if self.pdf.ship.doing_leaders {
            return Ok(());
        }
        let t = if struct_num.is_none() {
            OBJ_TYPE_DEST
        } else {
            OBJ_TYPE_STRUCT_DEST
        };
        let key = self.obj_id(id);
        let k = self.get_obj(t, key)?;
        if matches!(self.pdf.objs.get(k).aux, Aux::Dest(_)) {
            self.warn_dest_dup(id, b"ext4", b"has been already used, duplicate ignored");
            return Ok(());
        }
        let margin = self.dimen_par(PDF_DEST_MARGIN_CODE);
        let matrix = self.pdf.stacks.matrix_used();
        let (cur_h, cur_v) = (self.pdf.ship.cur_h, self.pdf.ship.cur_v);
        let mut rect = Rect::default();
        match kind {
            0 | 2 | 3 | 5 | 6 if matrix => {
                rect = self.set_rect_dimens(parent, x, y, dims, margin);
            }
            0 => {
                rect.left = cur_h;
                rect.top = cur_v;
            }
            2 | 5 => rect.top = cur_v,
            3 | 6 => rect.left = cur_h,
            7 => rect = self.set_rect_dimens(parent, x, y, dims, margin),
            _ => {}
        }
        self.pdf.objs.get_mut(k).aux = Aux::Dest(Box::new(Dest {
            rect,
            kind,
            zoom,
            struct_num,
            named: matches!(id, PdfId::Name(_)),
        }));
        self.pdf.ship.dest_list.push(k);
        Ok(())
    }

    /// `out_form` (the encoder's half).
    pub(crate) fn emit_form(&mut self, objnum: i32) -> Result<(), Jump> {
        self.pdf_end_text();
        self.pdf.out.print_ln(b"q");
        if !self.pdf.ship.xform_list.contains(&objnum) {
            self.pdf.ship.xform_list.push(objnum);
        }
        let (_, _, depth) = self.xform_dims(objnum);
        self.pdf.ship.cur_v += depth;
        self.pdf.out.print(b"1 0 0 1 ");
        let s = &self.pdf.ship;
        let (x, y) = (s.cur_h - s.origin_h, s.origin_v - s.cur_v);
        self.pdf_print_bp(x)?;
        self.pdf.out.out(b' ');
        self.pdf_print_bp(y)?;
        self.pdf.out.print_ln(b" cm");
        self.pdf.out.print(b"/Fm");
        let n = self.pdf.objs.get(objnum).info.num();
        self.pdf.out.print_int(i64::from(n));
        self.pdf_print_resname_prefix();
        self.pdf.out.print_ln(b" Do");
        self.pdf.out.print_ln(b"Q");
        Ok(())
    }

    /// `pdf_out_setmatrix`.
    fn pdf_out_setmatrix(&mut self, data: &partex_engine::node::Tokens) -> Result<(), Jump> {
        let mut s = self.tokens_string(data);
        let (h, v) = (
            self.pdf.ship.cur_h,
            self.pdf.ship.page_height - self.pdf.ship.cur_v,
        );
        if self.pdf.stacks.set_matrix(&s, h, v) {
            s.extend_from_slice(b" 0 0 cm");
            self.literal(&s, SET_ORIGIN, false)
        } else {
            self.pdf_error(b"\\pdfsetmatrix", b"Unrecognized format.")
        }
    }

    // ---- pdfTeX §750–§757: the list walks ----

    /// `pdf_hlist_out` or `pdf_vlist_out` of `b`.
    fn pdf_list_out(&mut self, b: &BoxNode) -> Result<(), Jump> {
        if b.vertical {
            self.pdf_vlist_out(b)
        } else {
            self.pdf_hlist_out(b)
        }
    }

    /// `output_one_char`.
    fn pdf_output_char(&mut self, f: i32, c: u8) -> Result<(), Jump> {
        self.pdf_output_char_at(f, c, 0)
    }

    /// A character of an hlist.
    fn pdf_char(&mut self, f: i32, c: u8) -> Result<(), Jump> {
        match self.fonts.get(f).glyph(i32::from(c)) {
            Some(g) => {
                self.pdf_output_char(f, c)?;
                self.pdf.ship.cur_h += g.width;
            }
            None => self.char_warning(f, i32::from(c))?,
        }
        Ok(())
    }

    /// pdfTeX §750: `pdf_hlist_out`.
    fn pdf_hlist_out(&mut self, this_box: &BoxNode) -> Result<(), Jump> {
        let mut glue = SetGlue::default();
        self.pdf.ship.cur_s += 1;
        let base_line = self.pdf.ship.cur_v;
        let left_edge = self.pdf.ship.cur_h;
        // create link annotations for the current hbox if needed
        for i in 0..self.pdf.ship.link_stack.len() {
            if self.pdf.ship.link_stack[i].level == self.pdf.ship.cur_s
                && !self.pdf.ship.no_running_link
            {
                self.append_link(this_box, left_edge, base_line, i)?;
            }
        }
        for p in &this_box.list {
            self.pdf_hnode(this_box, p, &mut glue, base_line, left_edge)?;
        }
        self.pdf.ship.cur_s -= 1;
        Ok(())
    }

    fn pdf_hnode(
        &mut self,
        this_box: &BoxNode,
        p: &Node,
        glue: &mut SetGlue,
        base_line: Scaled,
        left_edge: Scaled,
    ) -> Result<(), Jump> {
        match p {
            Node::Glyphs(g) => {
                let f = i32::from(g.font.0);
                for &c in g.chars() {
                    self.pdf_char(f, c)?;
                }
            }
            Node::Ligature(l) => self.pdf_char(i32::from(l.font.0), l.ch)?,
            Node::Box(b) => {
                if b.list.is_empty() {
                    self.pdf.ship.cur_h += b.width;
                } else {
                    self.pdf.ship.cur_v = base_line + b.shift;
                    let edge = self.pdf.ship.cur_h + b.width;
                    self.pdf_list_out(b)?;
                    self.pdf.ship.cur_h = edge;
                    self.pdf.ship.cur_v = base_line;
                }
            }
            Node::Rule {
                width,
                height,
                depth,
            } => self.pdf_hrule(this_box, *height, *depth, *width, base_line)?,
            Node::Whatsit(w) => self.pdf_whatsit(this_box, w, left_edge, base_line, false)?,
            Node::Glue { spec, .. } => {
                let wd = glue.set(this_box, spec);
                self.pdf.ship.cur_h += wd;
            }
            Node::Leaders(l) => {
                let rule_wd = glue.set(this_box, &l.spec);
                match &l.leader {
                    Node::Rule { height, depth, .. } => {
                        self.pdf_hrule(this_box, *height, *depth, rule_wd, base_line)?;
                    }
                    Node::Box(leader_box) => {
                        let leader_wd = leader_box.width;
                        if leader_wd > 0 && rule_wd > 0 {
                            let rule_wd = rule_wd + 10; // compensate for floating-point rounding
                            let edge = self.pdf.ship.cur_h + rule_wd;
                            let mut lx = 0;
                            let cur_h = self.pdf.ship.cur_h;
                            self.pdf.ship.cur_h = match l.kind {
                                Leaders::Aligned => {
                                    let h =
                                        left_edge + leader_wd * ((cur_h - left_edge) / leader_wd);
                                    if h < cur_h { h + leader_wd } else { h }
                                }
                                Leaders::Centered => cur_h + (rule_wd % leader_wd) / 2,
                                Leaders::Expanded => {
                                    let lq = rule_wd / leader_wd;
                                    let lr = rule_wd % leader_wd;
                                    lx = lr / (lq + 1);
                                    cur_h + (lr - (lq - 1) * lx) / 2
                                }
                            };
                            while self.pdf.ship.cur_h + leader_wd <= edge {
                                self.pdf.ship.cur_v = base_line + leader_box.shift;
                                let save_h = self.pdf.ship.cur_h;
                                let outer = self.pdf.ship.doing_leaders;
                                self.pdf.ship.doing_leaders = true;
                                self.pdf_list_out(leader_box)?;
                                self.pdf.ship.doing_leaders = outer;
                                self.pdf.ship.cur_v = base_line;
                                self.pdf.ship.cur_h = save_h + leader_wd + lx;
                            }
                            self.pdf.ship.cur_h = edge - 10;
                        } else {
                            self.pdf.ship.cur_h += rule_wd;
                        }
                    }
                    _ => return self.confusion(b"leaders"),
                }
            }
            Node::Kern { width, .. } | Node::MarginKern { width, .. } => {
                self.pdf.ship.cur_h += width;
            }
            Node::Math { width, subtype } => {
                if *subtype >= partex_engine::lr::L_CODE {
                    return self.pdf_error(
                        b"ext4",
                        b"TeXXeT in PDF output is not implemented in partex yet",
                    );
                }
                self.pdf.ship.cur_h += width;
            }
            Node::Disc(d) => {
                for q in &d.replace {
                    self.pdf_hnode(this_box, q, glue, base_line, left_edge)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// pdfTeX §753: a rule in an hlist, then move past it.
    fn pdf_hrule(
        &mut self,
        this_box: &BoxNode,
        height: Scaled,
        depth: Scaled,
        width: Scaled,
        base_line: Scaled,
    ) -> Result<(), Jump> {
        let ht = if height == RUNNING {
            this_box.height
        } else {
            height
        };
        let dp = if depth == RUNNING {
            this_box.depth
        } else {
            depth
        };
        let ht = ht + dp;
        if ht > 0 && width > 0 {
            self.pdf.ship.cur_v = base_line + dp;
            let (h, v) = (self.pdf.ship.cur_h, self.pdf.ship.cur_v);
            self.draw(Draw::Rule {
                x: h,
                y: v,
                w: width,
                h: ht,
            })?;
            self.pdf.ship.cur_v = base_line;
        }
        self.pdf.ship.cur_h += width;
        Ok(())
    }

    /// pdfTeX §757: `pdf_vlist_out`.
    fn pdf_vlist_out(&mut self, this_box: &BoxNode) -> Result<(), Jump> {
        let mut glue = SetGlue::default();
        self.pdf.ship.cur_s += 1;
        let left_edge = self.pdf.ship.cur_h;
        self.pdf.ship.cur_v -= this_box.height;
        let top_edge = self.pdf.ship.cur_v;
        for p in &this_box.list {
            match p {
                Node::Box(b) => {
                    if b.list.is_empty() {
                        self.pdf.ship.cur_v += b.height + b.depth;
                    } else {
                        self.pdf.ship.cur_v += b.height;
                        let save_v = self.pdf.ship.cur_v;
                        self.pdf.ship.cur_h = left_edge + b.shift;
                        self.pdf_list_out(b)?;
                        self.pdf.ship.cur_v = save_v + b.depth;
                        self.pdf.ship.cur_h = left_edge;
                    }
                }
                Node::Rule {
                    width,
                    height,
                    depth,
                } => self.pdf_vrule(this_box, *height + *depth, *width, left_edge)?,
                Node::Whatsit(w) => {
                    let y = top_edge + this_box.height;
                    self.pdf_whatsit(this_box, w, left_edge, y, true)?;
                }
                Node::Glue { spec, .. } => {
                    let ht = glue.set(this_box, spec);
                    self.pdf.ship.cur_v += ht;
                }
                Node::Leaders(l) => {
                    let rule_ht = glue.set(this_box, &l.spec);
                    match &l.leader {
                        Node::Rule { width, .. } => {
                            self.pdf_vrule(this_box, rule_ht, *width, left_edge)?;
                        }
                        Node::Box(leader_box) => {
                            let leader_ht = leader_box.height + leader_box.depth;
                            if leader_ht > 0 && rule_ht > 0 {
                                let rule_ht = rule_ht + 10;
                                let edge = self.pdf.ship.cur_v + rule_ht;
                                let mut lx = 0;
                                let cur_v = self.pdf.ship.cur_v;
                                self.pdf.ship.cur_v = match l.kind {
                                    Leaders::Aligned => {
                                        let v =
                                            top_edge + leader_ht * ((cur_v - top_edge) / leader_ht);
                                        if v < cur_v { v + leader_ht } else { v }
                                    }
                                    Leaders::Centered => cur_v + (rule_ht % leader_ht) / 2,
                                    Leaders::Expanded => {
                                        let lq = rule_ht / leader_ht;
                                        let lr = rule_ht % leader_ht;
                                        lx = lr / (lq + 1);
                                        cur_v + (lr - (lq - 1) * lx) / 2
                                    }
                                };
                                while self.pdf.ship.cur_v + leader_ht <= edge {
                                    self.pdf.ship.cur_h = left_edge + leader_box.shift;
                                    self.pdf.ship.cur_v += leader_box.height;
                                    let save_v = self.pdf.ship.cur_v;
                                    let outer = self.pdf.ship.doing_leaders;
                                    self.pdf.ship.doing_leaders = true;
                                    self.pdf_list_out(leader_box)?;
                                    self.pdf.ship.doing_leaders = outer;
                                    self.pdf.ship.cur_h = left_edge;
                                    self.pdf.ship.cur_v =
                                        save_v - leader_box.height + leader_ht + lx;
                                }
                                self.pdf.ship.cur_v = edge - 10;
                            } else {
                                self.pdf.ship.cur_v += rule_ht;
                            }
                        }
                        _ => return self.confusion(b"leaders"),
                    }
                }
                Node::Kern { width, .. } => self.pdf.ship.cur_v += width,
                Node::Glyphs(_) => return self.confusion(b"pdfvlistout"),
                _ => {}
            }
        }
        self.pdf.ship.cur_s -= 1;
        Ok(())
    }

    /// pdfTeX §758: a rule of thickness `ht` in a vlist.
    fn pdf_vrule(
        &mut self,
        this_box: &BoxNode,
        ht: Scaled,
        width: Scaled,
        left_edge: Scaled,
    ) -> Result<(), Jump> {
        let wd = if width == RUNNING {
            this_box.width
        } else {
            width
        };
        self.pdf.ship.cur_v += ht;
        if ht > 0 && wd > 0 {
            let (h, v) = (self.pdf.ship.cur_h, self.pdf.ship.cur_v);
            self.draw(Draw::Rule {
                x: h,
                y: v,
                w: wd,
                h: ht,
            })?;
            self.pdf.ship.cur_h = left_edge;
        }
        Ok(())
    }

    /// pdfTeX §1620–§1621: a whatsit in `pdf_hlist_out` (`x`, `y` the
    /// left edge and base line) or `pdf_vlist_out` (the left and top
    /// edges' bottom).
    fn pdf_whatsit(
        &mut self,
        this_box: &BoxNode,
        w: &Whatsit,
        x: Scaled,
        y: Scaled,
        vertical: bool,
    ) -> Result<(), Jump> {
        let p = match w {
            Whatsit::Open { .. } | Whatsit::Write { .. } | Whatsit::Close { .. } => {
                if !self.pdf.ship.doing_leaders {
                    self.out_what(w)?;
                }
                return Ok(());
            }
            Whatsit::Special { tokens } => {
                let s = self.tokens_string(tokens);
                return self.literal(&s, SCAN_SPECIAL, true);
            }
            Whatsit::LateSpecial { tokens } => {
                let s = self.late_text(tokens)?;
                return self.literal(&s, SCAN_SPECIAL, true);
            }
            Whatsit::Language { .. } => return Ok(()),
            Whatsit::Pdf(p) => &**p,
        };
        match p {
            PdfWhatsit::Literal { late, mode, data } => {
                let s = if *late {
                    self.late_text(data)?
                } else {
                    self.tokens_string(data)
                };
                self.literal(&s, i32::from(*mode), false)?;
            }
            PdfWhatsit::ColorStack { stack, cmd, data } => {
                self.pdf_out_colorstack(*stack, *cmd, data.as_ref())?;
            }
            PdfWhatsit::SetMatrix { data } => self.pdf_out_setmatrix(data)?,
            PdfWhatsit::Save => {
                let (h, v) = (self.pdf.ship.cur_h, self.pdf.ship.cur_v);
                self.pdf.stacks.save(h, v);
                self.literal(b"q", SET_ORIGIN, false)?;
            }
            PdfWhatsit::Restore => {
                let (h, v) = (self.pdf.ship.cur_h, self.pdf.ship.cur_v);
                if let Some(w) = self.pdf.stacks.restore(h, v) {
                    self.pdftex_warn(&w);
                }
                self.literal(b"Q", SET_ORIGIN, false)?;
            }
            PdfWhatsit::RefObj { objnum } => self.pdf.ship.obj_list.push(*objnum),
            PdfWhatsit::RefXForm { objnum, dims } => {
                if vertical {
                    self.pdf.ship.cur_v += dims.height;
                    let save_v = self.pdf.ship.cur_v;
                    self.pdf.ship.cur_h = x;
                    self.draw(Draw::Form { objnum: *objnum })?;
                    self.pdf.ship.cur_v = save_v + dims.depth;
                    self.pdf.ship.cur_h = x;
                } else {
                    self.pdf.ship.cur_v = y;
                    let edge = self.pdf.ship.cur_h;
                    self.draw(Draw::Form { objnum: *objnum })?;
                    self.pdf.ship.cur_h = edge + dims.width;
                    self.pdf.ship.cur_v = y;
                }
            }
            PdfWhatsit::RefXImage { objnum, dims } => {
                if vertical {
                    self.pdf.ship.cur_v += dims.height + dims.depth;
                    let save_v = self.pdf.ship.cur_v;
                    self.pdf.ship.cur_h = x;
                    self.draw(Draw::Image {
                        objnum: *objnum,
                        dims: *dims,
                    })?;
                    self.pdf.ship.cur_v = save_v;
                    self.pdf.ship.cur_h = x;
                } else {
                    self.pdf.ship.cur_v = y + dims.depth;
                    let edge = self.pdf.ship.cur_h;
                    self.draw(Draw::Image {
                        objnum: *objnum,
                        dims: *dims,
                    })?;
                    self.pdf.ship.cur_h = edge + dims.width;
                    self.pdf.ship.cur_v = y;
                }
            }
            PdfWhatsit::Annot { dims, data, objnum } => {
                self.do_annot(*dims, data, *objnum, this_box, x, y)?;
            }
            PdfWhatsit::StartLink {
                dims,
                attr,
                action,
                objnum,
            } => {
                if vertical {
                    return self.pdf_error(b"ext4", b"\\pdfstartlink ended up in vlist");
                }
                self.do_link(*dims, attr.as_ref(), action, *objnum, this_box, x, y)?;
            }
            PdfWhatsit::EndLink => {
                if vertical {
                    return self.pdf_error(b"ext4", b"\\pdfendlink ended up in vlist");
                }
                self.end_link()?;
            }
            PdfWhatsit::Dest {
                dims,
                struct_num,
                id,
                kind,
                zoom,
            } => self.do_dest(*dims, *struct_num, id, *kind, *zoom, this_box, x, y)?,
            PdfWhatsit::Thread { .. } | PdfWhatsit::EndThread => {
                return self.pdf_error(b"ext4", b"threads are not implemented in partex yet");
            }
            PdfWhatsit::SavePos => {
                let s = &self.pdf.ship;
                let y = if s.shipping_page {
                    s.page_height - s.cur_v
                } else {
                    s.xform_height + s.xform_depth - s.cur_v
                };
                let x = s.cur_h;
                self.set_pdf_last(crate::pdf::PdfLast::XPos, x);
                self.set_pdf_last(crate::pdf::PdfLast::YPos, y);
            }
            PdfWhatsit::SnapRefPoint => {}
            PdfWhatsit::SnapY { .. } | PdfWhatsit::SnapYComp { .. } => {
                if vertical {
                    return self.pdf_error(b"snapping", b"not implemented in partex yet");
                }
            }
            PdfWhatsit::InterwordSpaceOn => self.pdf.ship.faked_space = true,
            PdfWhatsit::InterwordSpaceOff => self.pdf.ship.faked_space = false,
            PdfWhatsit::FakeSpace => self.draw(Draw::FakeSpace)?,
            PdfWhatsit::RunningLinkOff => self.pdf.ship.no_running_link = true,
            PdfWhatsit::RunningLinkOn => self.pdf.ship.no_running_link = false,
        }
        Ok(())
    }

    // ---- pdfTeX §750, §760–§789: shipping a page or form ----

    /// pdfTeX §750's "Initialize variables for PDF output".
    fn init_pdf_output(&mut self) -> Result<(), Jump> {
        self.check_pdfversion()?;
        self.prepare_mag()?;
        let dd = self.int_par(PDF_DECIMAL_DIGITS_CODE).clamp(0, 4);
        self.pdf.out.fixed_decimal_digits = dd;
        let (q, _) = self.divide_scaled(
            ONE_HUNDRED_BP,
            TEN_POW[usize::try_from(dd).unwrap_or(0) + 2],
            0,
        )?;
        self.pdf.ship.min_bp_val = q;
        if self.int_par(PDF_UNIQUE_RESNAME_CODE) > 0 {
            return self.pdf_error(
                b"ext1",
                b"\\pdfuniqueresname is not implemented in partex yet",
            );
        }
        Ok(())
    }

    /// pdfTeX §750: `pdf_ship_out`: box `p` as a page, or (not
    /// `shipping_page`) as the form `pdf_cur_form`.
    ///
    /// Shipping reads and writes the writer's tables ([`SHIPPING`]).
    pub(crate) fn pdf_ship_out(&mut self, p: &BoxNode, shipping_page: bool) -> Result<(), Jump> {
        self.writer_scope(SHIPPING_READS, SHIPPING, |t| {
            if T::VALUES {
                t.pdf.ship.marking.clear();
            }
            let r = t.pdf_ship_out_now(p, shipping_page);
            t.glyphs_shipped();
            r
        })
    }

    /// The ship ends: the glyphs it used are its append (`Row::Glyphs`).
    fn glyphs_shipped(&mut self) {
        if !T::VALUES {
            return;
        }
        let n = self.pdf.ship.ships;
        self.pdf.ship.ships += 1;
        let g: Glyphs = core::mem::take(&mut self.pdf.ship.marking)
            .into_iter()
            .collect();
        let v = partex_ssa::Version::of(&g).0;
        let ix = n as usize;
        let gl = &mut self.pdf.ship.glyphs;
        if gl.len() <= ix {
            gl.resize(ix + 1, Arc::from(&[][..]));
        }
        gl[ix] = g;
        self.tracker.row_wrote(crate::track::Row::Glyphs(n), v);
    }

    /// The job's end: each font's glyphs used, the union of the ships'
    /// (with values; without, `mark_glyph` made them as it went).
    pub(crate) fn glyphs_union(&mut self) {
        if !T::VALUES {
            return;
        }
        let mut gl = Vec::with_capacity(self.pdf.ship.ships as usize);
        for n in 0..self.pdf.ship.ships {
            let g = self.pdf.ship.glyphs.get(n as usize).cloned();
            self.tracker.row_read(crate::track::Row::Glyphs(n), || {
                partex_ssa::Version::of(&g.clone().unwrap_or_else(|| Arc::from(&[][..]))).0
            });
            gl.extend(g);
        }
        let sets = glyph_union(&gl);
        // (a ship whose glyphs changed makes the end run again only if
        // this union changes: DESIGN 4.3, "The job's end")
        self.tracker.glyphs_united(partex_ssa::Version::of(&sets).0);
        for f in 0..self.pdf.ship.fonts.len() {
            let Ok(k) = i32::try_from(f) else { break };
            let c = sets.get(&k).copied().unwrap_or([0; 4]);
            if self.pdf.ship.fonts[f].chars != c {
                self.pdf.ship.fonts[f].chars = c;
            }
        }
    }

    fn pdf_ship_out_now(&mut self, p: &BoxNode, shipping_page: bool) -> Result<(), Jump> {
        // (a form's sealed lines, opened; a page's are already)
        let open = self.unsealed_deep(p);
        let p = open.as_ref().unwrap_or(p);
        let tracing_output = self.int_par(TRACING_OUTPUT_CODE);
        if tracing_output > 0 {
            self.print_nl(b"");
            self.print_ln();
            self.print_str(b"Completed box being shipped out");
        }
        if !self.pdf.ship.init {
            self.init_pdf_output()?;
            self.pdf.ship.init = true;
        }
        self.pdf.ship.shipping_page = shipping_page;
        if shipping_page {
            let (term_offset, file_offset) = self.offsets();
            if term_offset > self.params.max_print_line - 9 {
                self.print_ln();
            } else if term_offset > 0 || file_offset > 0 {
                self.print_char(b' ');
            }
            self.print_char(b'[');
            let mut j = 9;
            while self.count(j) == 0 && j > 0 {
                j -= 1;
            }
            for k in 0..=j {
                self.print_int(self.count(k));
                if k < j {
                    self.print_char(b'.');
                }
            }
            self.update_terminal();
        }
        if tracing_output > 0 {
            if shipping_page {
                self.print_char(b']');
            }
            self.begin_diagnostic();
            self.show_box_node(p);
            self.end_diagnostic(true);
        }
        self.pdf_ship_box_out(p, shipping_page)?;
        if self.etex_ex() {
            self.report_lr_problems();
        }
        if tracing_output <= 0 && shipping_page {
            self.print_char(b']');
        }
        self.set_dead_cycles(0);
        self.update_terminal();
        if self.int_par(TRACING_STATS_CODE) > 1 {
            let (var_used, dyn_used) = self.memory_usage();
            self.print_nl(b"Memory usage before: ");
            self.print_int(var_used);
            self.print_char(b'&');
            self.print_int(dyn_used);
            self.print_char(b';');
            self.print_str(b" after: ");
            self.print_int(var_used);
            self.print_char(b'&');
            self.print_int(dyn_used);
            self.print_str(b"; still untouched: ");
            self.print_int(0);
            self.print_ln();
        }
        Ok(())
    }

    /// pdfTeX §751: "Ship box `p` out".
    fn pdf_ship_box_out(&mut self, p: &BoxNode, shipping_page: bool) -> Result<(), Jump> {
        // §641: if the page is too large, `goto done`
        let v_offset = self.dimen_par(V_OFFSET_CODE);
        let h_offset = self.dimen_par(H_OFFSET_CODE);
        if p.height > MAX_DIMEN
            || p.depth > MAX_DIMEN
            || p.height + p.depth + v_offset > MAX_DIMEN
            || p.width + h_offset > MAX_DIMEN
        {
            self.print_err(b"Huge page cannot be shipped out");
            self.help(&[
                b"The page just created is more than 18 feet tall or",
                b"more than 18 feet wide, so I suspect something went wrong.",
            ]);
            self.error()?;
            if self.int_par(TRACING_OUTPUT_CODE) <= 0 {
                self.begin_diagnostic();
                self.print_nl(b"The following box has been deleted:");
                self.show_box_node(p);
                self.end_diagnostic(true);
            }
            return Ok(());
        }
        // pdfTeX §752: initialize variables as `pdf_ship_out` begins
        self.fix_pdfoutput()?;
        self.prepare_mag()?;
        let last_resources = self.pdf_new_objnum()?;
        self.pdf.ship.page_group_val = 0;
        let saved_lists = self.reset_resource_lists();
        if shipping_page {
            let h = self.dimen_par(PDF_H_ORIGIN_CODE) + h_offset;
            let v = self.dimen_par(PDF_V_ORIGIN_CODE) + v_offset;
            let pw = self.dimen_par(PDF_PAGE_WIDTH_CODE);
            let ph = self.dimen_par(PDF_PAGE_HEIGHT_CODE);
            let s = &mut *self.pdf.ship;
            s.h_offset = h;
            s.v_offset = v;
            s.page_width = if pw != 0 { pw } else { p.width + 2 * h };
            s.page_height = if ph != 0 {
                ph
            } else {
                p.height + p.depth + 2 * v
            };
            let n = self.pdf.ship.total_pages + 1;
            let page = self.get_obj(OBJ_TYPE_PAGE, Id::Num(n))?;
            self.pdf.ship.last_page = page;
            self.pdf.objs.get_mut(page).aux = Aux::Int(1);
            self.pdf.ship.last_stream = self.pdf_new_dict(OBJ_TYPE_OTHERS, 0, 0)?;
            let s = &mut *self.pdf.ship;
            s.cur_h = s.h_offset;
            s.cur_v = p.height + s.v_offset;
            s.origin_h = 0;
            s.origin_v = s.page_height;
            s.annot_list.clear();
            s.link_list.clear();
            s.dest_list.clear();
        } else {
            let form = self.pdf.ship.cur_form;
            let s = &mut *self.pdf.ship;
            s.xform_width = p.width;
            s.xform_height = p.height;
            s.xform_depth = p.depth;
            self.pdf_begin_dict(form, 0)?;
            let s = &mut *self.pdf.ship;
            s.last_stream = form;
            s.cur_v = p.height;
            s.cur_h = 0;
            s.origin_h = 0;
            s.origin_v = s.xform_height + s.xform_depth;
            // write out the form stream header
            self.pdf.out.print_ln(b"/Type /XObject");
            self.pdf.out.print_ln(b"/Subtype /Form");
            let attr = match &mut self.pdf.objs.get_mut(form).aux {
                Aux::XForm(x) => x.attr.take(),
                _ => None,
            };
            if let Some(a) = attr {
                self.pdf_print_toks_ln(&a);
            }
            self.pdf.out.print(b"/BBox [");
            self.pdf.out.print(b"0 0 ");
            self.pdf_print_bp(p.width)?;
            self.pdf.out.out(b' ');
            self.pdf_print_bp(p.height + p.depth)?;
            self.pdf.out.print_ln(b"]");
            self.pdf.out.print_ln(b"/FormType 1");
            self.pdf.out.print_ln(b"/Matrix [1 0 0 1 0 0]");
            self.pdf.out.indirect_ln(b"Resources", last_resources);
        }
        // start the stream of page/form contents
        self.pdf_begin_stream();
        self.pdf.ship.recording = true;
        let r = self.pdf_ship_contents(p, shipping_page);
        let flushed = self.flush_drawn();
        self.pdf.ship.recording = false;
        r?;
        flushed?;
        if !self.pdf.stacks.pos.is_empty() {
            let n = self.pdf.stacks.pos.len();
            let what = if shipping_page { "page" } else { "form" };
            let m = alloc::format!("{n} unmatched \\pdfsave after {what} shipout");
            return self.pdftex_fail(None, m.as_bytes());
        }
        self.pdf_end_stream();
        self.pdf_ship_resources(last_resources, p, shipping_page, saved_lists)
    }

    /// The contents of a page or form's stream, as display items.
    fn pdf_ship_contents(&mut self, p: &BoxNode, shipping_page: bool) -> Result<(), Jump> {
        if shipping_page {
            self.prepare_mag()?;
            let mag = self.int_par(MAG_CODE);
            if mag != 1000 {
                self.pdf.out.print_real(mag, 3);
                self.pdf.out.print(b" 0 0 ");
                self.pdf.out.print_real(mag, 3);
                self.pdf.out.print_ln(b" 0 0 cm");
            }
        }
        self.pdf.stacks.ship_begin(shipping_page);
        if shipping_page {
            self.pdf_out_colorstack_startpage()?;
        }
        self.pdf.ship.cur_s = -1;
        self.pdf_list_out(p)?;
        if shipping_page {
            self.pdf.ship.total_pages += 1;
        }
        self.pdf.ship.cur_s = -1;
        // finish shipping: finish the stream
        self.draw(Draw::EndText)
    }

    /// After a page or form's stream: its page object and the objects it
    /// uses.
    fn pdf_ship_resources(
        &mut self,
        last_resources: i32,
        p: &BoxNode,
        shipping_page: bool,
        saved_lists: (Vec<i32>, Vec<i32>, Vec<i32>, Vec<i32>, bool, i32),
    ) -> Result<(), Jump> {
        if shipping_page {
            self.write_page_object(last_resources, p)?;
        }
        // write out the resource lists: raw objects, images, forms
        let objs = self.pdf.ship.obj_list.clone();
        for k in objs {
            if !self.pdf.objs.is_written(k) {
                self.pdf_write_obj(k)?;
            }
        }
        let images = self.pdf.ship.ximage_list.clone();
        for k in images {
            if !self.pdf.objs.is_written(k) {
                self.pdf_write_image(k)?;
            }
        }
        let forms = self.pdf.ship.xform_list.clone();
        for k in forms {
            if !self.pdf.objs.is_written(k) {
                self.ship_form(k)?;
            }
        }
        if shipping_page {
            self.write_page_marks()?;
        }
        // write out the resources dictionary
        self.pdf_begin_dict(last_resources, 1)?;
        if shipping_page {
            if let Some(s) = self.toks_loc_string(PDF_PAGE_RESOURCES_LOC)
                && !s.is_empty()
            {
                self.pdf.out.print_ln(&s);
            }
        } else {
            let form = self.pdf.ship.cur_form;
            let res = match &mut self.pdf.objs.get_mut(form).aux {
                Aux::XForm(x) => x.resources.take(),
                _ => None,
            };
            if let Some(r) = res {
                self.pdf_print_toks_ln(&r);
            }
        }
        let fonts = self.pdf.ship.font_list.clone();
        if !fonts.is_empty() {
            self.pdf.out.print(b"/Font << ");
            for f in fonts {
                self.pdf.out.print(b"/F");
                let ff = self.pdf_ff(f);
                self.pdf_print_font_number(ff);
                self.pdf_print_resname_prefix();
                self.pdf.out.out(b' ');
                let n = self.pdf_font(ff).num;
                self.pdf.out.objnum(n);
                self.pdf.out.print(b" 0 R ");
            }
            self.pdf.out.print_ln(b">>");
            self.pdf.ship.text_procset = true;
        }
        let forms = self.pdf.ship.xform_list.clone();
        if !forms.is_empty() || !self.pdf.ship.ximage_list.is_empty() {
            self.pdf.out.print(b"/XObject << ");
            for k in forms {
                self.pdf.out.print(b"/Fm");
                let n = self.pdf.objs.get(k).info.num();
                self.pdf.out.print_int(i64::from(n));
                self.pdf_print_resname_prefix();
                self.pdf.out.out(b' ');
                self.pdf.out.objnum(k);
                self.pdf.out.print(b" 0 R ");
            }
            for k in self.pdf.ship.ximage_list.clone() {
                self.pdf.out.print(b"/Im");
                let n = self.pdf.objs.get(k).info.num();
                self.pdf.out.print_int(i64::from(n));
                self.pdf_print_resname_prefix();
                self.pdf.out.out(b' ');
                self.pdf.out.objnum(k);
                self.pdf.out.print(b" 0 R ");
                self.pdf.ship.image_procset |= self.image_color(k);
            }
            self.pdf.out.print_ln(b">>");
        }
        let omit = self.int_par(PDF_OMIT_PROCSET_CODE);
        if omit < 0 || (omit == 0 && self.pdf.out.fixed_major < 2) {
            self.pdf.out.print(b"/ProcSet [ /PDF");
            if self.pdf.ship.text_procset {
                self.pdf.out.print(b" /Text");
            }
            let ip = self.pdf.ship.image_procset;
            for (bit, name) in [
                (super::image::IMAGE_COLOR_B, &b" /ImageB"[..]),
                (super::image::IMAGE_COLOR_C, b" /ImageC"),
                (super::image::IMAGE_COLOR_I, b" /ImageI"),
            ] {
                if ip & bit != 0 {
                    self.pdf.out.print(name);
                }
            }
            self.pdf.out.print_ln(b" ]");
        }
        self.pdf_end_dict();
        self.restore_resource_lists(saved_lists);
        Ok(())
    }

    /// "Reset resource lists", returning the old ones (pdfTeX saves them
    /// only around forms; a page starts with them empty either way).
    fn reset_resource_lists(&mut self) -> (Vec<i32>, Vec<i32>, Vec<i32>, Vec<i32>, bool, i32) {
        let s = &mut *self.pdf.ship;
        (
            core::mem::take(&mut s.font_list),
            core::mem::take(&mut s.obj_list),
            core::mem::take(&mut s.xform_list),
            core::mem::take(&mut s.ximage_list),
            core::mem::replace(&mut s.text_procset, false),
            core::mem::replace(&mut s.image_procset, 0),
        )
    }

    fn restore_resource_lists(&mut self, l: (Vec<i32>, Vec<i32>, Vec<i32>, Vec<i32>, bool, i32)) {
        let s = &mut *self.pdf.ship;
        (
            s.font_list,
            s.obj_list,
            s.xform_list,
            s.ximage_list,
            s.text_procset,
            s.image_procset,
        ) = l;
    }

    /// "Write out pending forms": ship form `k`.
    fn ship_form(&mut self, k: i32) -> Result<(), Jump> {
        let saved = self.pdf.ship.cur_form;
        self.pdf.ship.cur_form = k;
        let b = match &mut self.pdf.objs.get_mut(k).aux {
            Aux::XForm(x) => x.boxed.take(),
            _ => None,
        };
        let Some(b) = b else {
            return self.confusion(b"pdf form");
        };
        // (the page's state the form walk changes)
        let s = &self.pdf.ship;
        let keep = (
            s.shipping_page,
            s.cur_h,
            s.cur_v,
            s.origin_h,
            s.origin_v,
            s.xform_width,
            s.xform_height,
            s.xform_depth,
        );
        self.pdf_ship_out(&b, false)?;
        let s = &mut *self.pdf.ship;
        (
            s.shipping_page,
            s.cur_h,
            s.cur_v,
            s.origin_h,
            s.origin_v,
            s.xform_width,
            s.xform_height,
            s.xform_depth,
        ) = keep;
        s.cur_form = saved;
        Ok(())
    }

    /// `\immediate\pdfxform`: ship the form just made.
    pub(crate) fn pdf_immediate_xform(&mut self) -> Result<(), Jump> {
        self.writer_scope(SHIPPING_READS, SHIPPING, Self::pdf_immediate_xform_now)
    }

    fn pdf_immediate_xform_now(&mut self) -> Result<(), Jump> {
        let k = self.pdf_last(crate::pdf::PdfLast::XForm);
        self.pdf.ship.cur_form = k;
        let b = match &mut self.pdf.objs.get_mut(k).aux {
            Aux::XForm(x) => x.boxed.take(),
            _ => None,
        };
        match b {
            Some(b) => self.pdf_ship_out(&b, false),
            None => self.confusion(b"pdf form"),
        }
    }

    /// `tokens_to_string` of a token list parameter, if not null.
    pub(crate) fn toks_loc_string(&mut self, loc: i32) -> Option<Vec<u8>> {
        let p = self.equiv_toks(loc).cloned()?;
        Some(self.printed(|t| t.token_show(&p)))
    }

    /// `pdf_print_toks_ln`.
    pub(crate) fn pdf_print_toks_ln(&mut self, t: &partex_engine::node::Tokens) {
        let s = self.tokens_string(t);
        if !s.is_empty() {
            self.pdf.out.print_ln(&s);
        }
    }

    /// `pdf_print_toks_ln` of a concatenated text.
    pub(crate) fn pdf_print_text_ln(&mut self, t: &[i32]) {
        let t: partex_engine::node::Tokens = partex_engine::node::TokenList::shared(t);
        self.pdf_print_toks_ln(&t);
    }

    /// pdfTeX §1554: `pdf_write_obj`.
    pub(crate) fn pdf_write_obj(&mut self, n: i32) -> Result<(), Jump> {
        self.writer_scope(SHIPPING_READS, SHIPPING, |t| t.pdf_write_obj_now(n))
    }

    fn pdf_write_obj_now(&mut self, n: i32) -> Result<(), Jump> {
        let o = match &mut self.pdf.objs.get_mut(n).aux {
            Aux::Obj(o) => {
                let kept = (**o).clone();
                // (`delete_toks`: the texts go once written)
                o.data = partex_engine::node::Tokens::default();
                o.stream_attr = None;
                kept
            }
            _ => return self.confusion(b"pdf_write_obj"),
        };
        let s = self.tokens_string(&o.data);
        if o.is_stream {
            self.pdf_begin_dict(n, 0)?;
            if let Some(a) = &o.stream_attr {
                self.pdf_print_toks_ln(a);
            }
            self.pdf_begin_stream();
        } else {
            self.pdf_begin_obj(n, 1)?;
        }
        if o.is_file {
            // (pdfTeX's `tex_b_openin`: web2c's `open_input` with
            // `kpse_tex_format`, so `\pdfobj file {t1.cmap}` is found on
            // TeX's search path)
            // (a load: a name the job stores reads its store, DESIGN 3.7)
            let found = self.read_source(&s);
            let Some(f) = found else {
                self.print_nl(b"! ");
                self.print_str(&s);
                self.print_str(b" not found.");
                return self.pdf_error(b"ext5", b"cannot open file for embedding");
            };
            self.print_str(b"<<");
            self.print_str(&s);
            if !f.contents.is_empty() {
                self.pdf.out.print(&f.contents);
                if !o.is_stream && self.pdf.out.last_in_buf().is_some_and(|c| c != 10) {
                    self.pdf.out.out(10);
                }
            }
            self.print_str(b">>");
        } else if o.is_stream {
            self.pdf.out.print(&s);
        } else {
            self.pdf.out.print_ln(&s);
        }
        if o.is_stream {
            self.pdf_end_stream();
        } else {
            self.pdf_end_obj();
        }
        Ok(())
    }

    /// pdfTeX §769: "Write out page object".
    fn write_page_object(&mut self, last_resources: i32, _p: &BoxNode) -> Result<(), Jump> {
        let page = self.pdf.ship.last_page;
        self.pdf_begin_dict(page, 1)?;
        self.pdf.out.print_ln(b"/Type /Page");
        let stream = self.pdf.ship.last_stream;
        self.pdf.out.indirect_ln(b"Contents", stream);
        self.pdf.out.indirect_ln(b"Resources", last_resources);
        let attr_text = self.toks_loc_string(PDF_PAGE_ATTR_LOC);
        let mediabox_given = attr_text
            .as_ref()
            .is_some_and(|s| substr_of_str(b"/MediaBox", s));
        if !mediabox_given {
            self.pdf.out.print(b"/MediaBox [0 0 ");
            let (w, h) = (self.pdf.ship.page_width, self.pdf.ship.page_height);
            self.pdf_print_mag_bp(w)?;
            self.pdf.out.out(b' ');
            self.pdf_print_mag_bp(h)?;
            self.pdf.out.print_ln(b"]");
        }
        if let Some(s) = attr_text
            && !s.is_empty()
        {
            self.pdf.out.print_ln(&s);
        }
        // generate the parent pages object
        if self.pdf.ship.total_pages % PAGES_TREE_KIDS_MAX == 1 {
            // (a leaf of the page tree, of pages 6k+1 to 6k+6: named by its
            // place in the tree, level 0 and index k, which an edit moves
            // only if the pages before it change in number)
            let k = self.pdf.ship.total_pages / PAGES_TREE_KIDS_MAX;
            let name = super::objtab::TREE_NAMES
                .load(core::sync::atomic::Ordering::Relaxed)
                .then(|| partex_engine::stablehash::StableHasher::of(&(b"pages", 0u8, k)));
            self.pdf.ship.last_pages =
                self.pdf_create_obj_named(OBJ_TYPE_PAGES, Id::Num(PAGES_TREE_KIDS_MAX), name)?;
        }
        let parent = self.pdf.ship.last_pages;
        self.pdf.out.indirect_ln(b"Parent", parent);
        if self.pdf.ship.page_group_val > 0 {
            self.pdf.out.print(b"/Group ");
            let g = self.pdf.ship.page_group_val;
            self.pdf.out.objnum(g);
            self.pdf.out.print_ln(b" 0 R");
        }
        let (annots, links) = (
            self.pdf.ship.annot_list.clone(),
            self.pdf.ship.link_list.clone(),
        );
        if !annots.is_empty() || !links.is_empty() {
            self.pdf.out.print(b"/Annots [ ");
            for k in annots.iter().chain(&links) {
                self.pdf.out.objnum(*k);
                self.pdf.out.print(b" 0 R ");
            }
            self.pdf.out.print_ln(b"]");
        }
        self.pdf_end_dict();
        Ok(())
    }

    /// pdfTeX §776–§780: "Write out pending PDF marks".
    fn write_page_marks(&mut self) -> Result<(), Jump> {
        self.pdf.ship.origin_h = 0;
        self.pdf.ship.origin_v = self.pdf.ship.page_height;
        for k in self.pdf.ship.annot_list.clone() {
            let Aux::Mark(m) = self.pdf.objs.get(k).aux.clone() else {
                return self.confusion(b"annot");
            };
            self.pdf_begin_dict(k, 1)?;
            self.pdf.out.print_ln(b"/Type /Annot");
            if let Some(d) = &m.data {
                self.pdf_print_toks_ln(d);
            }
            self.pdf_rectangle(m.rect)?;
            self.pdf_end_dict();
            self.forget_written(k);
        }
        for k in self.pdf.ship.link_list.clone() {
            let Aux::Mark(m) = self.pdf.objs.get(k).aux.clone() else {
                return self.confusion(b"link");
            };
            let action = m.action.clone().expect("a link's action");
            self.pdf_begin_dict(k, 1)?;
            self.pdf.out.print_ln(b"/Type /Annot");
            if action.kind != 3 {
                self.pdf.out.print_ln(b"/Subtype /Link");
            }
            if let Some(a) = &m.data {
                self.pdf_print_toks_ln(a);
            }
            self.pdf_rectangle(m.rect)?;
            if action.kind != 3 {
                self.pdf.out.print(b"/A ");
            }
            self.write_action(&action)?;
            self.pdf_end_dict();
            self.forget_written(k);
        }
        for k in self.pdf.ship.dest_list.clone() {
            if self.pdf.objs.is_written(k) {
                return self.pdf_error(
                    b"ext5",
                    b"destination has been already written (this shouldn't happen)",
                );
            }
            let Aux::Dest(d) = self.pdf.objs.get(k).aux.clone() else {
                return self.confusion(b"dest");
            };
            let dict = d.named && d.struct_num.is_none();
            if dict {
                self.pdf_begin_dict(k, 1)?;
                self.pdf.out.print(b"/D ");
            } else {
                self.pdf_begin_obj(k, 1)?;
            }
            self.pdf.out.out(b'[');
            let target = d.struct_num.unwrap_or(self.pdf.ship.last_page);
            self.pdf.out.objnum(target);
            self.pdf.out.print(b" 0 R ");
            let (oh, ov) = (self.pdf.ship.origin_h, self.pdf.ship.origin_v);
            let r = d.rect;
            match d.kind {
                0 => {
                    self.pdf.out.print(b"/XYZ ");
                    self.pdf_print_mag_bp(r.left - oh)?;
                    self.pdf.out.out(b' ');
                    self.pdf_print_mag_bp(ov - r.top)?;
                    self.pdf.out.out(b' ');
                    match d.zoom {
                        None => self.pdf.out.print(b"null"),
                        Some(z) => {
                            self.pdf.out.print_int(i64::from(z / 1000));
                            self.pdf.out.out(b'.');
                            self.pdf.out.print_int(i64::from(z % 1000));
                        }
                    }
                }
                1 => self.pdf.out.print(b"/Fit"),
                2 => {
                    self.pdf.out.print(b"/FitH ");
                    self.pdf_print_mag_bp(ov - r.top)?;
                }
                3 => {
                    self.pdf.out.print(b"/FitV ");
                    self.pdf_print_mag_bp(r.left - oh)?;
                }
                4 => self.pdf.out.print(b"/FitB"),
                5 => {
                    self.pdf.out.print(b"/FitBH ");
                    self.pdf_print_mag_bp(ov - r.top)?;
                }
                6 => {
                    self.pdf.out.print(b"/FitBV ");
                    self.pdf_print_mag_bp(r.left - oh)?;
                }
                7 => {
                    self.pdf.out.print(b"/FitR ");
                    self.pdf_print_rect_spec(r)?;
                }
                _ => return self.pdf_error(b"ext5", b"unknown dest type"),
            }
            self.pdf.out.print_ln(b"]");
            if dict {
                self.pdf_end_dict();
            } else {
                self.pdf_end_obj();
            }
            self.forget_written(k);
        }
        Ok(())
    }

    /// Drop what a written mark or destination kept for writing it (its
    /// place on the page, its data): nothing reads it again, and a state
    /// that keeps it differs from the previous build's after any edit
    /// that moved a link, so early cutoff would never match. (A
    /// destination stays one: duplicates are told by that.)
    fn forget_written(&mut self, k: i32) {
        let aux = &mut self.pdf.objs.get_mut(k).aux;
        *aux = match aux {
            Aux::Dest(_) => Aux::Dest(Box::new(Dest {
                rect: Rect::default(),
                kind: 0,
                zoom: None,
                struct_num: None,
                named: false,
            })),
            _ => Aux::None,
        };
    }

    /// pdfTeX §1618: `write_action`.
    pub(crate) fn write_action(&mut self, a: &Action) -> Result<(), Jump> {
        if a.kind == 3 {
            if let Some(t) = &a.tokens {
                self.pdf_print_toks_ln(t);
            }
            return Ok(());
        }
        self.pdf.out.print(b"<< ");
        if let Some(f) = &a.file {
            self.pdf.out.print(b"/F ");
            let s = self.tokens_string(f);
            if s.first() == Some(&b'(') && s.last() == Some(&b')') {
                self.pdf.out.print(&s);
            } else {
                self.pdf.out.print_str(&s);
            }
            self.pdf.out.print(b" ");
            if a.new_window > 0 {
                self.pdf.out.print(b"/NewWindow ");
                self.pdf.out.print(if a.new_window == 1 {
                    b"true ".as_slice()
                } else {
                    b"false "
                });
            }
        }
        let named = matches!(a.id, PdfId::Name(_));
        match a.kind {
            0 => {
                let page = a.id_num();
                if a.file.is_none() {
                    self.pdf.out.print(b"/S /GoTo /D [");
                    let k = self.get_obj(OBJ_TYPE_PAGE, Id::Num(page))?;
                    self.pdf.out.objnum(k);
                    self.pdf.out.print(b" 0 R");
                } else {
                    self.pdf.out.print(b"/S /GoToR /D [");
                    self.pdf.out.print_int(i64::from(page - 1));
                }
                self.pdf.out.out(b' ');
                let s = a
                    .tokens
                    .as_ref()
                    .map(|t| self.tokens_string(t))
                    .unwrap_or_default();
                self.pdf.out.print(&s);
                self.pdf.out.out(b']');
            }
            1 | 2 => {
                let t = if a.kind == 1 {
                    OBJ_TYPE_DEST
                } else {
                    OBJ_TYPE_THREAD
                };
                self.pdf.out.print(if a.kind == 1 {
                    if a.file.is_none() {
                        b"/S /GoTo ".as_slice()
                    } else {
                        b"/S /GoToR "
                    }
                } else {
                    b"/S /Thread "
                });
                let d = if a.file.is_none() {
                    let key = self.obj_id(&a.id);
                    self.get_obj(t, key)?
                } else {
                    0
                };
                if let PdfId::Name(n) = &a.id {
                    let s = self.tokens_string(n);
                    self.pdf.out.out(b'/');
                    self.pdf.out.print(b"D");
                    self.pdf.out.out(b' ');
                    self.pdf.out.print_str(&s);
                } else if a.file.is_none() {
                    self.pdf.out.indirect(b"D", d);
                } else if a.kind == 1 {
                    return self.pdf_error(
                        b"ext4",
                        b"`goto' option cannot be used with both `file' and `num'",
                    );
                } else {
                    self.pdf.out.out(b'/');
                    self.pdf.out.print(b"D");
                    self.pdf.out.out(b' ');
                    self.pdf.out.print_int(i64::from(a.id_num()));
                }
                let _ = named;
            }
            _ => {}
        }
        if let Some(sid) = &a.struct_id {
            self.pdf.out.out(b' ');
            if a.file.is_none() {
                let key = self.obj_id(sid);
                let k = self.get_obj(OBJ_TYPE_STRUCT_DEST, key)?;
                self.pdf.out.indirect(b"SD", k);
            } else {
                self.pdf.out.print(b"/SD ");
                if let PdfId::Name(n) = sid {
                    let s = self.tokens_string(n);
                    self.pdf.out.print(&s);
                }
            }
        }
        self.pdf.out.print_ln(b" >>");
        Ok(())
    }
}

/// `substr_of_str`: whether `s` occurs in `t` (not at its very end, as
/// pdfTeX's loop bound has it).
pub(crate) fn substr_of_str(s: &[u8], t: &[u8]) -> bool {
    if t.len() < s.len() {
        return false;
    }
    (0..t.len() - s.len()).any(|k| t[k..].starts_with(s))
}

trait ActionId {
    fn id_num(&self) -> i32;
}

impl ActionId for Action {
    fn id_num(&self) -> i32 {
        match self.id {
            PdfId::Num(n) => n,
            PdfId::Name(_) => 0,
        }
    }
}
