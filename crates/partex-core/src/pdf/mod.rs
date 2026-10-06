//! pdfTeX parts 32a–32f: pdfTeX's parameters and PDF output (pdfTeX
//! §672–§857). So far the constants and the INITEX defaults.

pub(crate) mod colorstack;
pub(crate) mod draw;
pub(crate) mod enc;
pub(crate) mod epdf;
pub(crate) mod ext;
pub(crate) mod finish;
pub(crate) mod image;
pub(crate) mod objtab;
pub(crate) mod out;
pub(crate) mod outline;
pub(crate) mod ship;
pub(crate) mod tounicode;
pub(crate) mod val;
pub(crate) mod vf;
pub mod vnum;
pub(crate) mod writefont;
pub(crate) mod writepng;
pub(crate) mod writet1;
pub mod xref;

use alloc::vec::Vec;

use partex_engine::node::Tokens;

use crate::arith::Scaled;
use crate::host::Host;
use crate::tex::Tex;
use crate::track::Tracker;
use crate::web::*;

/// pdfTeX §688: 1bp, 100bp and 100in in scaled points (constants in
/// pdfTeX's "set initial values").
pub(crate) const ONE_BP: Scaled = 65782; // 65781.76
pub(crate) const ONE_HUNDRED_BP: Scaled = 6578176;
pub(crate) const ONE_HUNDRED_INCH: Scaled = 473628672;

/// utils.c's `\pdfmatch` state: the searched string, `sub_match_count`
/// and, if the match succeeded, the groups.
pub(crate) type LastMatch = Option<(Vec<u8>, i32, Option<partex_engine::regex::Captures>)>;

val::record_by_hash!(LastMatch);

/// A text a token list concatenates to (`\pdfinfo` and its siblings).
pub(crate) type Toks = val::Val<Option<Vec<i32>>>;

/// pdfTeX's globals for PDF output that the front end touches: the
/// tables of DESIGN 7.17.12's `pdf` row, each a value carrying its
/// version (`val.rs`: a field per table).
#[derive(Clone, Debug, Default, PartialEq, Hash)]
pub(crate) struct PdfState {
    pub last_match: val::Val<LastMatch>,
    pub objs: objtab::ObjTab,
    pub out: val::Val<out::PdfOut>,
    /// `pdf_obj_count`, `pdf_xform_count`, `pdf_ximage_count`.
    pub obj_count: i32,
    pub xform_count: i32,
    pub ximage_count: i32,
    pub last_obj: i32,
    pub last_xform: i32,
    pub last_ximage: i32,
    pub last_ximage_pages: i32,
    pub last_ximage_colordepth: i32,
    pub last_annot: i32,
    pub last_link: i32,
    pub last_x_pos: i32,
    pub last_y_pos: i32,
    /// `pdf_retval`.
    pub retval: i32,
    /// `\pdfinfo`, `\pdfcatalog`, `\pdfnames`, `\pdftrailer`,
    /// `\pdftrailerid` texts, concatenated.
    pub info_toks: Toks,
    pub catalog_toks: Toks,
    pub catalog_openaction: i32,
    pub names_toks: Toks,
    pub trailer_toks: Toks,
    pub trailer_id_toks: Toks,
    pub first_outline: i32,
    pub last_outline: i32,
    pub parent_outline: i32,
    /// `pdf_space_font_name`.
    pub space_font_name: val::Val<Option<Vec<u8>>>,
    /// `pdf_font_attr`.
    pub font_attr: val::VMap<i32, Vec<u8>>,
    /// `pdf_font_nobuiltin_tounicode`.
    pub nobuiltin_tounicode: val::VSet<i32>,
    /// utils.c's color, position and matrix stacks.
    pub stacks: val::Val<colorstack::Stacks>,
    /// Shipping pages and forms.
    pub ship: ship::Ship,
    /// The font trees of writefont.c.
    pub fontw: val::Val<writefont::FontWriter>,
    /// pdftoepdf.cc's documents open for PDF inclusion.
    pub epdf: val::Val<epdf::EpdfDocs>,
    /// The writer scope open (`val.rs`; scratch, empty between calls).
    pub(crate) scope: val::Scope,
}

partex_engine::persist_struct!(PdfState {
    last_match,
    objs,
    out,
    obj_count,
    xform_count,
    ximage_count,
    last_obj,
    last_xform,
    last_ximage,
    last_ximage_pages,
    last_ximage_colordepth,
    last_annot,
    last_link,
    last_x_pos,
    last_y_pos,
    retval,
    info_toks,
    catalog_toks,
    catalog_openaction,
    names_toks,
    trailer_toks,
    trailer_id_toks,
    first_outline,
    last_outline,
    parent_outline,
    space_font_name,
    font_attr,
    nobuiltin_tounicode,
    stacks,
    ship,
    fontw,
    epdf,
    scope
});

impl PdfState {
    /// A hash of each field (`PARTEX_WATCH_DEBUG`: where two states differ).
    pub(crate) fn hash_parts(&self) -> Vec<(&'static str, u128)> {
        use core::hash::Hash;
        let h = |v: &dyn Fn(&mut partex_engine::stablehash::StableHasher)| {
            let mut s = partex_engine::stablehash::StableHasher::new();
            v(&mut s);
            s.finish128()
        };
        alloc::vec![
            ("last_match", h(&|s| self.last_match.hash(s))),
            ("objs", h(&|s| self.objs.hash(s))),
            ("out", h(&|s| self.out.hash(s))),
            ("obj_count", h(&|s| self.obj_count.hash(s))),
            ("xform_count", h(&|s| self.xform_count.hash(s))),
            ("ximage_count", h(&|s| self.ximage_count.hash(s))),
            ("last_obj", h(&|s| self.last_obj.hash(s))),
            ("last_xform", h(&|s| self.last_xform.hash(s))),
            ("last_ximage", h(&|s| self.last_ximage.hash(s))),
            ("last_ximage_pages", h(&|s| self.last_ximage_pages.hash(s))),
            (
                "last_ximage_colordepth",
                h(&|s| self.last_ximage_colordepth.hash(s))
            ),
            ("last_annot", h(&|s| self.last_annot.hash(s))),
            ("last_link", h(&|s| self.last_link.hash(s))),
            ("last_x_pos", h(&|s| self.last_x_pos.hash(s))),
            ("last_y_pos", h(&|s| self.last_y_pos.hash(s))),
            ("retval", h(&|s| self.retval.hash(s))),
            ("info_toks", h(&|s| self.info_toks.hash(s))),
            ("catalog_toks", h(&|s| self.catalog_toks.hash(s))),
            (
                "catalog_openaction",
                h(&|s| self.catalog_openaction.hash(s))
            ),
            ("names_toks", h(&|s| self.names_toks.hash(s))),
            ("trailer_toks", h(&|s| self.trailer_toks.hash(s))),
            ("trailer_id_toks", h(&|s| self.trailer_id_toks.hash(s))),
            ("first_outline", h(&|s| self.first_outline.hash(s))),
            ("last_outline", h(&|s| self.last_outline.hash(s))),
            ("parent_outline", h(&|s| self.parent_outline.hash(s))),
            ("space_font_name", h(&|s| self.space_font_name.hash(s))),
            ("font_attr", h(&|s| self.font_attr.hash(s))),
            (
                "nobuiltin_tounicode",
                h(&|s| self.nobuiltin_tounicode.hash(s))
            ),
            ("stacks", h(&|s| self.stacks.hash(s))),
            ("ship", h(&|s| self.ship.hash(s))),
            ("fontw", h(&|s| self.fontw.hash(s))),
            ("epdf", h(&|s| self.epdf.hash(s))),
        ]
    }
}

/// The values pdfTeX's `\pdflast…` and `\pdfretval` read (pdfTeX §447's
/// `last_item` codes), each written by the command that makes its
/// object (or by `\pdfsavepos` at shipout): what a machine keeps as
/// cells of their own (`MCell::PdfLast`), read where they are read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PdfLast {
    Obj = 0,
    XForm,
    XImage,
    XImagePages,
    XImageColordepth,
    Annot,
    Link,
    XPos,
    YPos,
    Retval,
}

impl PdfLast {
    /// Every one, in the order of their numbers.
    pub const ALL: [PdfLast; 10] = [
        PdfLast::Obj,
        PdfLast::XForm,
        PdfLast::XImage,
        PdfLast::XImagePages,
        PdfLast::XImageColordepth,
        PdfLast::Annot,
        PdfLast::Link,
        PdfLast::XPos,
        PdfLast::YPos,
        PdfLast::Retval,
    ];
}

impl PdfState {
    /// The value `k` becomes `v`.
    pub(crate) fn set_last(&mut self, k: PdfLast, v: i32) {
        *match k {
            PdfLast::Obj => &mut self.last_obj,
            PdfLast::XForm => &mut self.last_xform,
            PdfLast::XImage => &mut self.last_ximage,
            PdfLast::XImagePages => &mut self.last_ximage_pages,
            PdfLast::XImageColordepth => &mut self.last_ximage_colordepth,
            PdfLast::Annot => &mut self.last_annot,
            PdfLast::Link => &mut self.last_link,
            PdfLast::XPos => &mut self.last_x_pos,
            PdfLast::YPos => &mut self.last_y_pos,
            PdfLast::Retval => &mut self.retval,
        } = v;
    }

    /// The value `k`.
    pub(crate) fn last(&self, k: PdfLast) -> i32 {
        match k {
            PdfLast::Obj => self.last_obj,
            PdfLast::XForm => self.last_xform,
            PdfLast::XImage => self.last_ximage,
            PdfLast::XImagePages => self.last_ximage_pages,
            PdfLast::XImageColordepth => self.last_ximage_colordepth,
            PdfLast::Annot => self.last_annot,
            PdfLast::Link => self.last_link,
            PdfLast::XPos => self.last_x_pos,
            PdfLast::YPos => self.last_y_pos,
            PdfLast::Retval => self.retval,
        }
    }

    pub(crate) fn last_mut(&mut self, k: PdfLast) -> &mut i32 {
        match k {
            PdfLast::Obj => &mut self.last_obj,
            PdfLast::XForm => &mut self.last_xform,
            PdfLast::XImage => &mut self.last_ximage,
            PdfLast::XImagePages => &mut self.last_ximage_pages,
            PdfLast::XImageColordepth => &mut self.last_ximage_colordepth,
            PdfLast::Annot => &mut self.last_annot,
            PdfLast::Link => &mut self.last_link,
            PdfLast::XPos => &mut self.last_x_pos,
            PdfLast::YPos => &mut self.last_y_pos,
            PdfLast::Retval => &mut self.retval,
        }
    }
}

/// The PDF state as a machine's `Rest` hashes it: without the
/// [`PdfLast`] values if `last`, and without the writer's words
/// ([`word`]: the object lists' heads, if the object table's entries are
/// cells, the outlines' first, last and parent, the catalog's open
/// action) if `words`; in the derived order otherwise.
pub(crate) struct WithoutCells<'a> {
    pub pdf: &'a PdfState,
    pub last: bool,
    pub words: bool,
}

impl core::hash::Hash for WithoutCells<'_> {
    fn hash<S: core::hash::Hasher>(&self, h: &mut S) {
        let PdfState {
            last_match,
            objs,
            out,
            obj_count,
            xform_count,
            ximage_count,
            last_obj,
            last_xform,
            last_ximage,
            last_ximage_pages,
            last_ximage_colordepth,
            last_annot,
            last_link,
            last_x_pos,
            last_y_pos,
            retval,
            info_toks,
            catalog_toks,
            catalog_openaction,
            names_toks,
            trailer_toks,
            trailer_id_toks,
            first_outline,
            last_outline,
            parent_outline,
            space_font_name,
            font_attr,
            nobuiltin_tounicode,
            stacks,
            ship,
            fontw,
            epdf,
            scope: _,
        } = self.pdf;
        last_match.hash(h);
        if self.words {
            objs.hash_without_heads(h);
        } else {
            objs.hash(h);
        }
        (out, obj_count, xform_count, ximage_count).hash(h);
        if !self.last {
            (last_obj, last_xform, last_ximage, last_ximage_pages).hash(h);
            (last_ximage_colordepth, last_annot, last_link).hash(h);
            (last_x_pos, last_y_pos, retval).hash(h);
        }
        (info_toks, catalog_toks, names_toks).hash(h);
        (trailer_toks, trailer_id_toks).hash(h);
        if !self.words {
            (
                catalog_openaction,
                first_outline,
                last_outline,
                parent_outline,
            )
                .hash(h);
        }
        (space_font_name, font_attr, nobuiltin_tounicode).hash(h);
        (stacks, ship, fontw, epdf).hash(h);
    }
}

/// The PDF writer's words that are a machine's cells of their own
/// (`MCell::PdfWord`, `statehash::PDF_WORD_CELLS`): the object lists'
/// heads (`HEADS` of them, type `t` is word `t`, while the object
/// table's entries are cells), the outlines' first, last and parent, and
/// the catalog's open action. Each is read and written by a few routines
/// only (`\pdfoutline`, `\pdfcatalog`, an object made of a listed type,
/// the job's end), and none of them is read by the text in between.
pub mod word {
    /// The object lists' heads: words `0..HEADS`.
    pub const HEADS: u8 = 11;
    pub const FIRST_OUTLINE: u8 = 11;
    pub const LAST_OUTLINE: u8 = 12;
    pub const PARENT_OUTLINE: u8 = 13;
    pub const CATALOG_OPENACTION: u8 = 14;
    /// The words end here.
    pub const COUNT: u8 = 15;
}

impl PdfState {
    /// Word `k` ([`word`]).
    #[must_use]
    pub fn word(&self, k: u8) -> i32 {
        match k {
            word::FIRST_OUTLINE => self.first_outline,
            word::LAST_OUTLINE => self.last_outline,
            word::PARENT_OUTLINE => self.parent_outline,
            word::CATALOG_OPENACTION => self.catalog_openaction,
            t => self.objs.head[usize::from(t)],
        }
    }

    /// Word `k` ([`word`]), to set.
    pub fn word_mut(&mut self, k: u8) -> &mut i32 {
        match k {
            word::FIRST_OUTLINE => &mut self.first_outline,
            word::LAST_OUTLINE => &mut self.last_outline,
            word::PARENT_OUTLINE => &mut self.parent_outline,
            word::CATALOG_OPENACTION => &mut self.catalog_openaction,
            t => &mut self.objs.head[usize::from(t)],
        }
    }
}

/// Literal modes (pdfTeX §695).
pub(crate) const SET_ORIGIN: i32 = 0;
pub(crate) const DIRECT_PAGE: i32 = 1;
pub(crate) const DIRECT_ALWAYS: i32 = 2;
pub(crate) const SCAN_SPECIAL: i32 = 3;

/// A text a token list concatenates to (`concat_tokens`).
pub(crate) fn concat(to: &mut Option<Vec<i32>>, t: &Tokens) {
    to.get_or_insert_with(Vec::new).extend_from_slice(t);
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// [`PdfLast`] value `k`, read (the tracker is told).
    pub(crate) fn pdf_last(&self, k: PdfLast) -> i32 {
        self.tracker.pdf_last_access(k as u8, false);
        self.writer_read(val::field::LAST + k as u8);
        self.pdf.last(k)
    }

    /// Set [`PdfLast`] value `k` (the tracker is told).
    pub(crate) fn set_pdf_last(&mut self, k: PdfLast, v: i32) {
        self.tracker.pdf_last_access(k as u8, true);
        *self.pdf.last_mut(k) = v;
        self.writer_wrote(val::field::LAST + k as u8);
    }

    /// Object `k`'s `obj_aux`, read.
    pub(crate) fn pdf_obj_aux(&self, k: i32) -> &objtab::Aux {
        // (with virtual numbers, the entry's own slot: `get` logs it)
        if !self.pdf.objs.ssa.on {
            self.writer_read(val::field::OBJS);
        }
        &self.pdf.objs.get(k).aux
    }

    /// A form's width, height and depth (`obj_xform_width` etc.).
    pub(crate) fn xform_dims(&self, k: i32) -> (Scaled, Scaled, Scaled) {
        match self.pdf_obj_aux(k) {
            objtab::Aux::XForm(x) => (x.width, x.height, x.depth),
            _ => (0, 0, 0),
        }
    }

    /// An image's width, height and depth.
    pub(crate) fn ximage_dims(&self, k: i32) -> (Scaled, Scaled, Scaled) {
        match self.pdf_obj_aux(k) {
            objtab::Aux::XImage(x) => (x.width, x.height, x.depth),
            _ => (0, 0, 0),
        }
    }

    /// pdfTeX's INITEX table entries for its parameters (pdfTeX §672,
    /// §1064); in effect for every flavor, unobservable in TeX.
    pub(crate) fn init_pdftex_params(&mut self) {
        self.set_dimen_par(PDF_H_ORIGIN_CODE, (ONE_HUNDRED_INCH + 50) / 100);
        self.set_dimen_par(PDF_V_ORIGIN_CODE, (ONE_HUNDRED_INCH + 50) / 100);
        self.set_int_par(PDF_COMPRESS_LEVEL_CODE, 9);
        self.set_int_par(PDF_OBJCOMPRESSLEVEL_CODE, 0);
        self.set_int_par(PDF_DECIMAL_DIGITS_CODE, 3);
        self.set_int_par(PDF_IMAGE_RESOLUTION_CODE, 72);
        self.set_int_par(PDF_MAJOR_VERSION_CODE, 1);
        self.set_int_par(PDF_MINOR_VERSION_CODE, 4);
        self.set_int_par(PDF_GAMMA_CODE, 1000);
        self.set_int_par(PDF_IMAGE_GAMMA_CODE, 2200);
        self.set_int_par(PDF_IMAGE_HICOLOR_CODE, 1);
        self.set_int_par(PDF_IMAGE_APPLY_GAMMA_CODE, 0);
        self.set_dimen_par(PDF_PX_DIMEN_CODE, ONE_BP);
        self.set_int_par(PDF_DRAFTMODE_CODE, 0);
        // pdfTeX §1064
        self.set_dimen_par(PDF_IGNORED_DIMEN_CODE, crate::nest::IGNORE_DEPTH);
        for c in [
            PDF_EACH_LINE_HEIGHT_CODE,
            PDF_EACH_LINE_DEPTH_CODE,
            PDF_FIRST_LINE_HEIGHT_CODE,
            PDF_LAST_LINE_DEPTH_CODE,
        ] {
            self.set_dimen_par(c, crate::nest::IGNORE_DEPTH);
        }
    }
}
