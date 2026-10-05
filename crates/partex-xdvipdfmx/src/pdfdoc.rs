//! pdfdoc.c, pdfdoc.h: the document (catalog, page tree, outlines,
//! articles, name trees, annotations, form XObjects being grabbed).
//!
//! C's `static pdf_doc pdoc` is [`State::pdoc`]; the static functions
//! that took `pdf_doc *p` are methods of [`Dpx`] without it. Pages,
//! outline items and articles live in vectors and are named by index.

use crate::dpxutil::HtTable;
use crate::pdfcolor::PdfColor;
use crate::pdfdev::{PdfRect, PdfTmatrix};
use crate::prelude::*;

/// A name tree (pdfnames.c).
pub use crate::pdfnames::NameTree;

/// `enum pdf_page_boundary`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum PdfPageBoundary {
    /// `pdf_page_boundary__auto`.
    #[default]
    Auto = 0,
    MediaBox = 1,
    CropBox = 2,
    ArtBox = 3,
    TrimBox = 4,
    BleedBox = 5,
}

/// `PDF_DOC_GRABBING_NEST_MAX`.
pub const PDF_DOC_GRABBING_NEST_MAX: i32 = 4;
/// `PDFDOC_PAGES_ALLOC_SIZE`.
pub const PDFDOC_PAGES_ALLOC_SIZE: u32 = 128;
/// `PDFDOC_ARTICLE_ALLOC_SIZE`.
pub const PDFDOC_ARTICLE_ALLOC_SIZE: i32 = 16;
/// `PDFDOC_BEAD_ALLOC_SIZE`.
pub const PDFDOC_BEAD_ALLOC_SIZE: i32 = 16;
/// `USE_MY_MEDIABOX` (a page's `flags`).
pub const USE_MY_MEDIABOX: i32 = 1 << 0;
/// `PAGE_CLUSTER` (`build_page_tree`'s fan-out).
pub const PAGE_CLUSTER: i32 = 4;
/// `BOOKMARKS_OPEN_DEFAULT`.
pub const BOOKMARKS_OPEN_DEFAULT: i32 = 0;
/// `MAX_OUTLINE_DEPTH`.
pub const MAX_OUTLINE_DEPTH: u32 = 256;
/// `TITLE_STRING` (an empty bookmark's title).
pub const TITLE_STRING: &[u8] = b"<No Title>";
/// `JS_CODE` (an empty bookmark's action).
pub const JS_CODE: &[u8] =
    b"app.alert(\"The author of this document made this bookmark item empty!\", 3, 0)";
/// `name_dict_categories`.
pub const NAME_DICT_CATEGORIES: [&[u8]; 10] = [
    b"Dests",
    b"AP",
    b"JavaScript",
    b"Pages",
    b"Templates",
    b"IDS",
    b"URLS",
    b"EmbeddedFiles",
    b"AlternatePresentations",
    b"Renditions",
];

/// `struct pdf_dev_setting`.
#[derive(Clone, Debug, Default)]
pub struct PdfDevSetting {
    /// Conversion unit.
    pub dvi2pts: f64,
    /// Number of decimal digits kept.
    pub precision: i32,
    /// 1 for black or white.
    pub ignore_colors: i32,
}

/// `struct pdf_enc_setting` (encryption is not ported; kept for the shape).
#[derive(Clone, Debug, Default)]
pub struct PdfEncSetting {
    pub key_size: i32,
    pub permission: u32,
    /// User password.
    pub uplain: Vec<u8>,
    /// Owner password.
    pub oplain: Vec<u8>,
    pub use_aes: i32,
    pub encrypt_metadata: i32,
}

/// `struct pdf_obj_setting`.
#[derive(Clone, Debug, Default)]
pub struct PdfObjSetting {
    pub enable_objstm: i32,
    pub enable_predictor: i32,
    pub compression_level: i32,
}

/// `struct pdf_setting`.
#[derive(Clone, Debug, Default)]
pub struct PdfSetting {
    pub ver_major: i32,
    pub ver_minor: i32,
    pub media_width: f64,
    pub media_height: f64,
    /// `annot_grow_amount.x`, `.y`.
    pub annot_grow_amount: (f64, f64),
    pub outline_open_depth: i32,
    pub check_gotos: i32,
    pub enable_manual_thumb: i32,
    pub enable_encrypt: i32,
    pub encrypt: PdfEncSetting,
    pub device: PdfDevSetting,
    pub object: PdfObjSetting,
}

/// `pdf_form`.
#[derive(Clone, Debug, Default)]
pub struct PdfForm {
    pub ident: Vec<u8>,
    pub matrix: PdfTmatrix,
    pub cropbox: PdfRect,
    pub resources: Option<Obj>,
    pub contents: Option<Obj>,
}

/// `struct form_list_node` (`prev` is the order in [`PdfDoc::pending_forms`]).
#[derive(Clone, Debug, Default)]
pub struct FormListNode {
    pub q_depth: i32,
    pub form: PdfForm,
}

/// `pdf_page`.
#[derive(Clone, Debug, Default)]
pub struct PdfPage {
    pub page_obj: Option<Obj>,
    pub page_ref: Option<Obj>,
    pub flags: i32,
    pub ref_x: f64,
    pub ref_y: f64,
    pub cropbox: PdfRect,
    pub resources: Option<Obj>,
    /// Contents.
    pub background: Option<Obj>,
    pub contents: Option<Obj>,
    /// Global bop, background, contents, global eop.
    pub content_refs: [Option<Obj>; 4],
    pub annots: Option<Obj>,
    pub beads: Option<Obj>,
}

/// `pdf_olitem`: `first`, `parent`, `next` are indices into
/// [`Outlines::items`].
#[derive(Clone, Debug, Default)]
pub struct PdfOlitem {
    pub dict: Option<Obj>,
    pub is_open: i32,
    pub first: Option<usize>,
    pub parent: Option<usize>,
    pub next: Option<usize>,
}

/// `pdf_bead`.
#[derive(Clone, Debug, Default)]
pub struct PdfBead {
    pub id: Option<Vec<u8>>,
    pub page_no: i32,
    pub rect: PdfRect,
}

/// `pdf_article` (`num_beads`/`max_beads` are `beads.len()`).
#[derive(Clone, Debug, Default)]
pub struct PdfArticle {
    pub id: Vec<u8>,
    pub info: Option<Obj>,
    pub beads: Vec<PdfBead>,
}

/// `struct name_dict`.
#[derive(Clone, Debug)]
pub struct NameDict {
    pub category: &'static [u8],
    pub data: Option<NameTree>,
}

/// `pdf_doc.root`.
#[derive(Clone, Debug, Default)]
pub struct DocRoot {
    pub dict: Option<Obj>,
    pub viewerpref: Option<Obj>,
    pub pagelabels: Option<Obj>,
    pub pages: Option<Obj>,
    pub names: Option<Obj>,
    pub threads: Option<Obj>,
}

/// `pdf_doc.pages`: `entries` has `max_entries` (allocated, default)
/// pages, of which the first `num_entries` are begun; page `n` (1-based)
/// is `entries[n - 1]`.
#[derive(Clone, Debug, Default)]
pub struct DocPages {
    pub mediabox: PdfRect,
    pub bop: Option<Obj>,
    pub eop: Option<Obj>,
    /// Not actually the total number of pages.
    pub num_entries: i32,
    pub max_entries: i32,
    pub entries: Vec<PdfPage>,
}

/// `pdf_doc.outlines`: the items in an arena.
#[derive(Clone, Debug, Default)]
pub struct Outlines {
    pub items: Vec<PdfOlitem>,
    pub first: Option<usize>,
    pub current: Option<usize>,
    pub current_depth: i32,
}

/// `pdf_doc.options`.
#[derive(Clone, Debug, Default)]
pub struct DocOptions {
    pub outline_open_depth: i32,
    /// `annot_grow.x`, `.y`.
    pub annot_grow: (f64, f64),
    pub enable_manual_thumb: i32,
}

/// `pdf_doc`.
#[derive(Clone, Debug, Default)]
pub struct PdfDoc {
    pub root: DocRoot,
    pub info: Option<Obj>,
    pub pages: DocPages,
    pub outlines: Outlines,
    /// `articles.entries` (`num_entries`/`max_entries` are the length).
    pub articles: Vec<PdfArticle>,
    /// `NUM_NAME_CATEGORY` entries (C's NULL terminator dropped).
    pub names: Vec<NameDict>,
    pub check_gotos: i32,
    /// Destination name → its replacement name object (values released
    /// at close, C's `pdf_release_obj` free function).
    pub gotos: HtTable<Obj>,
    pub options: DocOptions,
    /// The stack of forms being grabbed: the top (C's head) is the last.
    pub pending_forms: Vec<FormListNode>,
    pub thumb_basename: Option<Vec<u8>>,
}

/// The `breaking_state` static.
#[derive(Clone, Debug, Default)]
pub struct BreakingState {
    pub dirty: i32,
    pub broken: i32,
    pub annot_dict: Option<Obj>,
    pub rect: PdfRect,
}

/// `pdf_boxes`: the boxes and resources found for a page of a PDF file.
#[derive(Clone, Debug, Default)]
pub struct PdfBoxes {
    pub page_tree: Option<Obj>,
    pub resources: Option<Obj>,
    pub rotate: Option<Obj>,
    pub art_box: Option<Obj>,
    pub trim_box: Option<Obj>,
    pub bleed_box: Option<Obj>,
    pub media_box: Option<Obj>,
    pub crop_box: Option<Obj>,
}

/// pdfdoc.c's statics and globals.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// `pdoc`.
    pub pdoc: PdfDoc,
    /// `global_names` (pdfdoc.h: named objects of specials and images).
    pub global_names: Option<NameTree>,
    /// `breaking_state`.
    pub breaking_state: BreakingState,
    /// `bgcolor` (C initializes it to gray 1; `pdf_open_document` sets it
    /// to white with `pdf_doc_set_bgcolor(NULL)`).
    pub bgcolor: PdfColor,
}

/// `find_bead`: the bead's index in `article.beads`.
fn find_bead(article: &PdfArticle, bead_id: &[u8]) -> Option<usize> {
    todo!()
}

/// `warn_undef_dests`: warns of each goto with no destination.
fn warn_undef_dests(dests: &NameTree, gotos: &HtTable<Obj>) {
    todo!()
}

impl Dpx {
    /// `read_thumbnail`: the image's reference.
    fn read_thumbnail(&mut self, thumb_filename: &[u8]) -> Option<Obj> {
        todo!()
    }

    /// `pdf_doc_init_catalog`.
    fn pdf_doc_init_catalog(&mut self) {
        todo!()
    }
    /// `pdf_doc_close_catalog`.
    fn pdf_doc_close_catalog(&mut self) {
        todo!()
    }

    /// `doc_resize_page_entries`.
    fn doc_resize_page_entries(&mut self, size: i32) {
        todo!()
    }
    /// `doc_get_page_entry`: the index of page `page_no` in
    /// `pdoc.pages.entries` (grown as needed).
    fn doc_get_page_entry(&mut self, page_no: u32) -> usize {
        todo!()
    }

    /// `pdf_doc_set_bop_content`.
    pub fn pdf_doc_set_bop_content(&mut self, content: &[u8]) {
        todo!()
    }
    /// `pdf_doc_set_eop_content`.
    pub fn pdf_doc_set_eop_content(&mut self, content: &[u8]) {
        todo!()
    }

    /// `pdf_doc_init_docinfo`.
    fn pdf_doc_init_docinfo(&mut self) {
        todo!()
    }
    /// `pdf_doc_close_docinfo`.
    fn pdf_doc_close_docinfo(&mut self) {
        todo!()
    }

    /// `pdf_doc_get_page_resources`.
    fn pdf_doc_get_page_resources(&mut self, category: &[u8]) -> Option<Obj> {
        todo!()
    }
    /// `pdf_doc_add_page_resource`.
    pub fn pdf_doc_add_page_resource(
        &mut self,
        category: &[u8],
        resource_name: &[u8],
        resource_ref: Obj,
    ) {
        todo!()
    }

    /// `doc_flush_page`: page `entries[page]`.
    fn doc_flush_page(&mut self, page: usize, parent_ref: Obj) {
        todo!()
    }
    /// `build_page_tree`: pages `entries[firstpage..firstpage+num_pages]`.
    fn build_page_tree(
        &mut self,
        firstpage: usize,
        num_pages: i32,
        parent_ref: Option<Obj>,
    ) -> Obj {
        todo!()
    }
    /// `pdf_doc_init_page_tree`.
    fn pdf_doc_init_page_tree(&mut self, media_width: f64, media_height: f64) {
        todo!()
    }
    /// `pdf_doc_close_page_tree`.
    fn pdf_doc_close_page_tree(&mut self) {
        todo!()
    }

    /// `pdf_doc_get_page_count`: of the PDF file `pf` (pdfread's index).
    pub fn pdf_doc_get_page_count(&mut self, pf: u32) -> i32 {
        todo!()
    }
    /// `set_bounding_box`: status (0 ok) and the box.
    fn set_bounding_box(
        &mut self,
        opt_bbox: PdfPageBoundary,
        media_box: Option<Obj>,
        crop_box: Option<Obj>,
        art_box: Option<Obj>,
        trim_box: Option<Obj>,
        bleed_box: Option<Obj>,
    ) -> (i32, PdfRect) {
        todo!()
    }
    /// `set_transform_matrix`: status (0 ok) and the matrix; `bbox` is
    /// changed (rotated) in place, as in C.
    fn set_transform_matrix(
        &mut self,
        bbox: &mut PdfRect,
        rotate: Option<Obj>,
    ) -> (i32, PdfTmatrix) {
        todo!()
    }
    /// `get_page_properties`.
    fn get_page_properties(&mut self, boxes: &mut PdfBoxes) {
        todo!()
    }
    /// `page_by_name`.
    fn page_by_name(&mut self, catalog: Obj, page_name: &[u8], boxes: &mut PdfBoxes) {
        todo!()
    }
    /// `page_by_number`.
    fn page_by_number(&mut self, page_tree: Option<Obj>, page_no: i32, boxes: &mut PdfBoxes) {
        todo!()
    }
    /// `pdf_doc_get_page`: the page object (none on error), its box, its
    /// matrix, and its resources (linked; only when `want_resources`, C's
    /// non-NULL `resources_p`).
    pub fn pdf_doc_get_page(
        &mut self,
        pf: u32,
        page_no: i32,
        page_name: Option<&[u8]>,
        opt_bbox: PdfPageBoundary,
        want_resources: bool,
    ) -> (Option<Obj>, PdfRect, PdfTmatrix, Option<Obj>) {
        todo!()
    }

    /// `pdf_doc_init_bookmarks`.
    fn pdf_doc_init_bookmarks(&mut self, bm_open_depth: i32) {
        todo!()
    }
    /// `clean_bookmarks`: the item and its siblings and children.
    fn clean_bookmarks(&mut self, item: Option<usize>) -> i32 {
        todo!()
    }
    /// `flush_bookmarks`: the count.
    fn flush_bookmarks(&mut self, node: usize, parent_ref: Obj, parent_dict: Obj) -> i32 {
        todo!()
    }
    /// `pdf_doc_bookmarks_up`.
    pub fn pdf_doc_bookmarks_up(&mut self) -> i32 {
        todo!()
    }
    /// `pdf_doc_bookmarks_down`.
    pub fn pdf_doc_bookmarks_down(&mut self) -> i32 {
        todo!()
    }
    /// `pdf_doc_bookmarks_depth`.
    pub fn pdf_doc_bookmarks_depth(&mut self) -> i32 {
        todo!()
    }
    /// `pdf_doc_bookmarks_add` (`is_open` < 0: by the open depth).
    pub fn pdf_doc_bookmarks_add(&mut self, dict: Obj, is_open: i32) {
        todo!()
    }
    /// `pdf_doc_close_bookmarks`.
    fn pdf_doc_close_bookmarks(&mut self) {
        todo!()
    }

    /// `pdf_doc_init_names`.
    fn pdf_doc_init_names(&mut self, check_gotos: i32) {
        todo!()
    }
    /// `pdf_doc_add_names`: 0, or -1 for an unknown category / error.
    pub fn pdf_doc_add_names(&mut self, category: &[u8], key: &[u8], value: Obj) -> i32 {
        todo!()
    }
    /// `pdf_doc_add_goto`.
    fn pdf_doc_add_goto(&mut self, annot_dict: Obj) {
        todo!()
    }
    /// `pdf_doc_close_names`.
    fn pdf_doc_close_names(&mut self) {
        todo!()
    }

    /// `pdf_doc_add_annot`.
    pub fn pdf_doc_add_annot(
        &mut self,
        page_no: u32,
        rect: &PdfRect,
        annot_dict: Obj,
        new_annot: i32,
    ) {
        todo!()
    }

    /// `pdf_doc_init_articles`.
    fn pdf_doc_init_articles(&mut self) {
        todo!()
    }
    /// `pdf_doc_begin_article`.
    pub fn pdf_doc_begin_article(&mut self, article_id: &[u8], article_info: Option<Obj>) {
        todo!()
    }
    /// `pdf_doc_add_bead` (spc_pdfm passes no bead id).
    pub fn pdf_doc_add_bead(
        &mut self,
        article_id: &[u8],
        bead_id: Option<&[u8]>,
        page_no: i32,
        rect: &PdfRect,
    ) {
        todo!()
    }
    /// `make_article`: article `pdoc.articles[article]`.
    fn make_article(
        &mut self,
        article: usize,
        bead_ids: Option<&[&[u8]]>,
        article_info: Option<Obj>,
    ) -> Option<Obj> {
        todo!()
    }
    /// `clean_article`: article `pdoc.articles[article]`.
    fn clean_article(&mut self, article: usize) {
        todo!()
    }
    /// `pdf_doc_close_articles`.
    fn pdf_doc_close_articles(&mut self) {
        todo!()
    }

    /// `pdf_doc_set_mediabox` (page 0: the root).
    pub fn pdf_doc_set_mediabox(&mut self, page_no: u32, mediabox: &PdfRect) {
        todo!()
    }
    /// `pdf_doc_get_mediabox`.
    fn pdf_doc_get_mediabox(&mut self, page_no: u32) -> PdfRect {
        todo!()
    }
    /// `pdf_doc_current_page_resources`.
    pub fn pdf_doc_current_page_resources(&mut self) -> Option<Obj> {
        todo!()
    }
    /// `pdf_doc_get_dictionary` (errors on an unknown category).
    pub fn pdf_doc_get_dictionary(&mut self, category: &[u8]) -> Obj {
        todo!()
    }
    /// `pdf_doc_current_page_number`.
    pub fn pdf_doc_current_page_number(&mut self) -> i32 {
        todo!()
    }
    /// `pdf_doc_ref_page`.
    pub fn pdf_doc_ref_page(&mut self, page_no: u32) -> Option<Obj> {
        todo!()
    }
    /// `pdf_doc_get_reference` (`@THISPAGE`, `@PREVPAGE`, `@NEXTPAGE`).
    pub fn pdf_doc_get_reference(&mut self, category: &[u8]) -> Option<Obj> {
        todo!()
    }

    /// `pdf_doc_page_tree()`.
    pub fn pdf_doc_page_tree(&mut self) -> Obj {
        self.pdf_doc_get_dictionary(b"Pages")
    }
    /// `pdf_doc_catalog()`.
    pub fn pdf_doc_catalog(&mut self) -> Obj {
        self.pdf_doc_get_dictionary(b"Catalog")
    }
    /// `pdf_doc_docinfo()`.
    pub fn pdf_doc_docinfo(&mut self) -> Obj {
        self.pdf_doc_get_dictionary(b"Info")
    }
    /// `pdf_doc_names()`.
    pub fn pdf_doc_names(&mut self) -> Obj {
        self.pdf_doc_get_dictionary(b"Names")
    }
    /// `pdf_doc_this_page()`.
    pub fn pdf_doc_this_page(&mut self) -> Obj {
        self.pdf_doc_get_dictionary(b"@THISPAGE")
    }
    /// `pdf_doc_this_page_ref()`.
    pub fn pdf_doc_this_page_ref(&mut self) -> Option<Obj> {
        self.pdf_doc_get_reference(b"@THISPAGE")
    }
    /// `pdf_doc_next_page_ref()`.
    pub fn pdf_doc_next_page_ref(&mut self) -> Option<Obj> {
        self.pdf_doc_get_reference(b"@NEXTPAGE")
    }
    /// `pdf_doc_prev_page_ref()`.
    pub fn pdf_doc_prev_page_ref(&mut self) -> Option<Obj> {
        self.pdf_doc_get_reference(b"@PREVPAGE")
    }

    /// `pdf_doc_new_page`.
    fn pdf_doc_new_page(&mut self) {
        todo!()
    }
    /// `pdf_doc_finish_page`.
    fn pdf_doc_finish_page(&mut self) {
        todo!()
    }
    /// `pdf_doc_set_bgcolor` (none: white).
    pub fn pdf_doc_set_bgcolor(&mut self, color: Option<&PdfColor>) {
        todo!()
    }
    /// `doc_fill_page_background`.
    fn doc_fill_page_background(&mut self) {
        todo!()
    }
    /// `pdf_doc_begin_page`.
    pub fn pdf_doc_begin_page(&mut self, scale: f64, x_origin: f64, y_origin: f64) {
        todo!()
    }
    /// `pdf_doc_end_page`.
    pub fn pdf_doc_end_page(&mut self) {
        todo!()
    }
    /// `pdf_doc_add_page_content`.
    pub fn pdf_doc_add_page_content(&mut self, buffer: &[u8]) {
        todo!()
    }

    /// `pdf_open_document`: `filename` none is stdout; encryption
    /// (`pdf_out_set_encrypt`) is not ported (`settings.enable_encrypt`
    /// must be 0).
    pub fn pdf_open_document(
        &mut self,
        filename: Option<&[u8]>,
        creator: Option<&[u8]>,
        id1: &[u8; 16],
        id2: &[u8; 16],
        settings: PdfSetting,
    ) {
        todo!()
    }
    /// `pdf_close_document`.
    pub fn pdf_close_document(&mut self) {
        todo!()
    }

    /// `pdf_doc_make_xform`.
    fn pdf_doc_make_xform(
        &mut self,
        xform: Obj,
        bbox: &PdfRect,
        matrix: Option<&PdfTmatrix>,
        resources: Option<Obj>,
        attrib: Option<Obj>,
    ) {
        todo!()
    }
    /// `pdf_doc_begin_grabbing`: the xobj_id of the form started.
    pub fn pdf_doc_begin_grabbing(
        &mut self,
        ident: &[u8],
        ref_x: f64,
        ref_y: f64,
        cropbox: &PdfRect,
    ) -> i32 {
        todo!()
    }
    /// `pdf_doc_end_grabbing`.
    pub fn pdf_doc_end_grabbing(&mut self, attrib: Option<Obj>) {
        todo!()
    }

    /// `reset_box`.
    fn reset_box(&mut self) {
        todo!()
    }
    /// `pdf_doc_begin_annot`.
    pub fn pdf_doc_begin_annot(&mut self, dict: Obj) {
        todo!()
    }
    /// `pdf_doc_end_annot`.
    pub fn pdf_doc_end_annot(&mut self) {
        todo!()
    }
    /// `pdf_doc_break_annot`.
    pub fn pdf_doc_break_annot(&mut self) {
        todo!()
    }
    /// `pdf_doc_expand_box`.
    pub fn pdf_doc_expand_box(&mut self, rect: &PdfRect) {
        todo!()
    }
}
