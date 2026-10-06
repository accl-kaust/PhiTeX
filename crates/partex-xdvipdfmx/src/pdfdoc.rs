//! pdfdoc.c, pdfdoc.h: the document (catalog, page tree, outlines,
//! articles, name trees, annotations, form XObjects being grabbed).
//!
//! C's `static pdf_doc pdoc` is [`State::pdoc`]; the static functions
//! that took `pdf_doc *p` are methods of [`Dpx`] without it. Pages,
//! outline items and articles live in vectors and are named by index.

use crate::dpxutil::HtTable;
use crate::fmt::round_acc;
use crate::obj::{PDF_ARRAY, STREAM_COMPRESS};
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
pub const PDFDOC_PAGES_ALLOC_SIZE: usize = 128;
/// `PDFDOC_ARTICLE_ALLOC_SIZE`.
pub const PDFDOC_ARTICLE_ALLOC_SIZE: i32 = 16;
/// `PDFDOC_BEAD_ALLOC_SIZE`.
pub const PDFDOC_BEAD_ALLOC_SIZE: i32 = 16;
/// `USE_MY_MEDIABOX` (a page's `flags`).
pub const USE_MY_MEDIABOX: i32 = 1 << 0;
/// `PAGE_CLUSTER` (`build_page_tree`'s fan-out).
pub const PAGE_CLUSTER: usize = 4;
/// `BOOKMARKS_OPEN_DEFAULT`.
pub const BOOKMARKS_OPEN_DEFAULT: i32 = 0;
/// `MAX_OUTLINE_DEPTH`.
pub const MAX_OUTLINE_DEPTH: i32 = 256;
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
    pub num_entries: usize,
    pub max_entries: usize,
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

impl Dpx {
    fn doc_resize_page_entries(&mut self, size: usize) {
        if size > self.doc.pdoc.pages.entries.len() {
            self.doc
                .pdoc
                .pages
                .entries
                .resize_with(size, PdfPage::default);
        }
    }

    fn doc_get_page_entry(&mut self, page_no: u32) -> usize {
        let page_no = page_no as usize;
        assert!(page_no <= 65535, "Page number too large!");
        assert!(page_no != 0, "Invalid Page number");
        if page_no > self.doc.pdoc.pages.entries.len() {
            self.doc_resize_page_entries(page_no + PDFDOC_PAGES_ALLOC_SIZE);
        }
        page_no - 1
    }

    fn pdf_doc_init_catalog(&mut self) {
        self.doc.pdoc.root = DocRoot::default();
        let d = self.o.new_dict();
        self.doc.pdoc.root.dict = Some(d);
        self.o.set_root(d);
    }

    fn pdf_doc_close_catalog(&mut self) {
        let root = self.doc.pdoc.root.dict.expect("catalog");
        if let Some(vp) = self.doc.pdoc.root.viewerpref.take() {
            let tmp = self.o.lookup_dict(root, b"ViewerPreferences");
            match tmp {
                None => {
                    let r = self.o.ref_obj(vp);
                    self.o.put(root, b"ViewerPreferences", r);
                }
                Some(t) if self.o.is_dict(Some(t)) => {
                    self.o.merge_dict(vp, t);
                    let r = self.o.ref_obj(vp);
                    self.o.put(root, b"ViewerPreferences", r);
                }
                Some(_) => {}
            }
            self.o.release(vp);
        }
        if let Some(pl) = self.doc.pdoc.root.pagelabels.take() {
            if self.o.lookup_dict(root, b"PageLabels").is_none() {
                let tmp = self.o.new_dict();
                let l = self.o.link(pl);
                self.o.put(tmp, b"Nums", l);
                let r = self.o.ref_obj(tmp);
                self.o.put(root, b"PageLabels", r);
                self.o.release(tmp);
            }
            self.o.release(pl);
        }
        self.o.put_name(root, b"Type", b"Catalog");
        self.o.release(root);
        self.doc.pdoc.root.dict = None;
    }

    /// `pdf_doc_set_bop_content`.
    pub fn pdf_doc_set_bop_content(&mut self, content: &[u8]) {
        if let Some(b) = self.doc.pdoc.pages.bop.take() {
            self.o.release(b);
        }
        if !content.is_empty() {
            let s = self.o.new_stream(STREAM_COMPRESS);
            self.o.add_stream(s, content);
            self.doc.pdoc.pages.bop = Some(s);
        }
    }

    /// `pdf_doc_set_eop_content`.
    pub fn pdf_doc_set_eop_content(&mut self, content: &[u8]) {
        if let Some(b) = self.doc.pdoc.pages.eop.take() {
            self.o.release(b);
        }
        if !content.is_empty() {
            let s = self.o.new_stream(STREAM_COMPRESS);
            self.o.add_stream(s, content);
            self.doc.pdoc.pages.eop = Some(s);
        }
    }

    fn pdf_doc_init_docinfo(&mut self) {
        let d = self.o.new_dict();
        self.doc.pdoc.info = Some(d);
        self.o.set_info(d);
    }

    fn pdf_doc_close_docinfo(&mut self) {
        let docinfo = self.doc.pdoc.info.expect("docinfo");
        for key in [
            &b"Title"[..],
            b"Author",
            b"Subject",
            b"Keywords",
            b"Creator",
            b"Producer",
            b"CreationDate",
            b"ModDate",
        ] {
            if let Some(v) = self.o.lookup_dict(docinfo, key)
                && (!self.o.is_string(Some(v)) || self.o.string_length(v) == 0)
            {
                self.o.remove_dict(docinfo, key);
            }
        }
        if self.o.lookup_dict(docinfo, b"Producer").is_none() {
            let mut banner = self.session.my_name.clone();
            banner.extend_from_slice(b" (");
            banner.extend_from_slice(crate::session::VERSION);
            banner.push(b')');
            self.o.put_string(docinfo, b"Producer", &banner);
        }
        if self.o.lookup_dict(docinfo, b"CreationDate").is_none() {
            let now = self.dpx_util_format_asn_date(true);
            self.o.put_string(docinfo, b"CreationDate", &now);
        }
        self.o.release(docinfo);
        self.doc.pdoc.info = None;
    }

    fn pdf_doc_get_page_resources(&mut self, category: &[u8]) -> Result<Option<Obj>> {
        let res_dict = if let Some(f) = self.doc.pdoc.pending_forms.last() {
            match f.form.resources {
                Some(r) => r,
                None => {
                    let r = self.o.new_dict();
                    self.doc
                        .pdoc
                        .pending_forms
                        .last_mut()
                        .expect("form")
                        .form
                        .resources = Some(r);
                    r
                }
            }
        } else {
            let i = self.doc.pdoc.pages.num_entries;
            self.doc_resize_page_entries(i + 1);
            match self.doc.pdoc.pages.entries[i].resources {
                Some(r) => r,
                None => {
                    let r = self.o.new_dict();
                    self.doc.pdoc.pages.entries[i].resources = Some(r);
                    r
                }
            }
        };
        match self.o.lookup_dict(res_dict, category) {
            None => {
                let r = self.o.new_dict();
                self.o.put(res_dict, category, r);
                Ok(Some(r))
            }
            Some(r) if self.o.is_indirect(Some(r)) => {
                let d = self.o.deref_obj(Some(r))?;
                if let Some(d) = d {
                    self.o.release(d);
                }
                Ok(d)
            }
            Some(r) => Ok(Some(r)),
        }
    }

    /// `pdf_doc_add_page_resource`: takes `resource_ref`.
    pub fn pdf_doc_add_page_resource(
        &mut self,
        category: &[u8],
        resource_name: &[u8],
        resource_ref: Obj,
    ) -> Result<()> {
        let Some(resources) = self.pdf_doc_get_page_resources(category)? else {
            return Ok(());
        };
        if self.o.lookup_dict(resources, resource_name).is_some() {
            self.o.release(resource_ref);
        } else {
            self.o.put(resources, resource_name, resource_ref);
        }
        Ok(())
    }

    fn doc_flush_page(&mut self, i: usize, parent_ref: Obj) {
        let page_obj = self.doc.pdoc.pages.entries[i].page_obj.expect("page");
        self.o.put_name(page_obj, b"Type", b"Page");
        self.o.put(page_obj, b"Parent", parent_ref);
        if self.doc.pdoc.pages.entries[i].flags & USE_MY_MEDIABOX != 0 {
            let c = self.doc.pdoc.pages.entries[i].cropbox;
            let mb = self.o.new_array();
            for v in [c.llx, c.lly, c.urx, c.ury] {
                let n = self.o.new_number(round_acc(v, 0.01));
                self.o.add_array(mb, n);
            }
            self.o.put(page_obj, b"MediaBox", mb);
        }
        let contents_array = self.o.new_array();
        let refs = core::mem::take(&mut self.doc.pdoc.pages.entries[i].content_refs);
        if let Some(r) = refs[0] {
            self.o.add_array(contents_array, r);
        } else if let Some(bop) = self.doc.pdoc.pages.bop
            && self.o.stream_length(bop) > 0
        {
            let r = self.o.ref_obj(bop);
            self.o.add_array(contents_array, r);
        }
        if let Some(r) = refs[1] {
            self.o.add_array(contents_array, r);
        }
        if let Some(r) = refs[2] {
            self.o.add_array(contents_array, r);
        }
        if let Some(r) = refs[3] {
            self.o.add_array(contents_array, r);
        } else if let Some(eop) = self.doc.pdoc.pages.eop
            && self.o.stream_length(eop) > 0
        {
            let r = self.o.ref_obj(eop);
            self.o.add_array(contents_array, r);
        }
        self.o.put(page_obj, b"Contents", contents_array);
        if let Some(a) = self.doc.pdoc.pages.entries[i].annots.take() {
            let r = self.o.ref_obj(a);
            self.o.put(page_obj, b"Annots", r);
            self.o.release(a);
        }
        if let Some(b) = self.doc.pdoc.pages.entries[i].beads.take() {
            let r = self.o.ref_obj(b);
            self.o.put(page_obj, b"B", r);
            self.o.release(b);
        }
        self.o.release(page_obj);
        let pr = self.doc.pdoc.pages.entries[i].page_ref.take();
        self.o.release_opt(pr);
        self.doc.pdoc.pages.entries[i].page_obj = None;
    }

    fn build_page_tree(&mut self, first: usize, num_pages: usize, parent_ref: Option<Obj>) -> Obj {
        let this = self.o.new_dict();
        let self_ref = if parent_ref.is_some() {
            self.o.ref_obj(this)
        } else {
            let pages = self.doc.pdoc.root.pages.expect("pages");
            self.o.ref_obj(pages)
        };
        self.o.put_name(this, b"Type", b"Pages");
        self.o.put_number(this, b"Count", num_pages as f64);
        if let Some(p) = parent_ref {
            self.o.put(this, b"Parent", p);
        }
        let kids = self.o.new_array();
        if num_pages > 0 && num_pages <= PAGE_CLUSTER {
            for i in 0..num_pages {
                let k = first + i;
                self.page_kid(k, kids, self_ref);
            }
        } else if num_pages > 0 {
            for i in 0..PAGE_CLUSTER {
                let start = (i * num_pages) / PAGE_CLUSTER;
                let end = ((i + 1) * num_pages) / PAGE_CLUSTER;
                if end - start > 1 {
                    let l = self.o.link(self_ref);
                    let sub = self.build_page_tree(first + start, end - start, Some(l));
                    let r = self.o.ref_obj(sub);
                    self.o.add_array(kids, r);
                    self.o.release(sub);
                } else {
                    self.page_kid(first + start, kids, self_ref);
                }
            }
        }
        self.o.put(this, b"Kids", kids);
        self.o.release(self_ref);
        this
    }

    fn page_kid(&mut self, k: usize, kids: Obj, self_ref: Obj) {
        if self.doc.pdoc.pages.entries[k].page_ref.is_none() {
            let po = self.doc.pdoc.pages.entries[k].page_obj.expect("page");
            self.doc.pdoc.pages.entries[k].page_ref = Some(self.o.ref_obj(po));
        }
        let pr = self.doc.pdoc.pages.entries[k].page_ref.expect("page ref");
        let l = self.o.link(pr);
        self.o.add_array(kids, l);
        let p = self.o.link(self_ref);
        self.doc_flush_page(k, p);
    }

    fn pdf_doc_init_page_tree(&mut self, media_width: f64, media_height: f64) {
        self.doc.pdoc.root.pages = Some(self.o.new_dict());
        self.doc.pdoc.pages.num_entries = 0;
        self.doc.pdoc.pages.entries.clear();
        self.doc.pdoc.pages.bop = None;
        self.doc.pdoc.pages.eop = None;
        self.doc.pdoc.pages.mediabox = PdfRect {
            llx: 0.0,
            lly: 0.0,
            urx: media_width,
            ury: media_height,
        };
    }

    fn pdf_doc_close_page_tree(&mut self) {
        let count = self.doc.pdoc.pages.num_entries;
        for page_no in count + 1..=self.doc.pdoc.pages.entries.len() {
            let i = page_no - 1;
            if self.doc.pdoc.pages.entries[i].page_obj.is_some() {
                let r = self.doc.pdoc.pages.entries[i].page_ref.take();
                self.o.release_opt(r);
            }
            if let Some(po) = self.doc.pdoc.pages.entries[i].page_obj.take() {
                self.o.release(po);
            }
            if let Some(a) = self.doc.pdoc.pages.entries[i].annots.take() {
                self.o.release(a);
            }
            if let Some(b) = self.doc.pdoc.pages.entries[i].beads.take() {
                self.o.release(b);
            }
            if let Some(r) = self.doc.pdoc.pages.entries[i].resources.take() {
                self.o.release(r);
            }
        }
        let root_tree = self.build_page_tree(0, count, None);
        let pages = self.doc.pdoc.root.pages.expect("pages");
        self.o.merge_dict(pages, root_tree);
        self.o.release(root_tree);
        if let Some(b) = self.doc.pdoc.pages.bop.take() {
            self.o.add_stream(b, b"\n");
            self.o.release(b);
        }
        if let Some(e) = self.doc.pdoc.pages.eop.take() {
            self.o.add_stream(e, b"\n");
            self.o.release(e);
        }
        let mb = self.o.new_array();
        let m = self.doc.pdoc.pages.mediabox;
        for v in [m.llx, m.lly, m.urx, m.ury] {
            let n = self.o.new_number(round_acc(v, 0.01));
            self.o.add_array(mb, n);
        }
        self.o.put(pages, b"MediaBox", mb);
        let root = self.doc.pdoc.root.dict.expect("catalog");
        let r = self.o.ref_obj(pages);
        self.o.put(root, b"Pages", r);
        self.o.release(pages);
        self.doc.pdoc.root.pages = None;
        self.doc.pdoc.pages.entries.clear();
        self.doc.pdoc.pages.num_entries = 0;
    }

    fn pdf_doc_init_bookmarks(&mut self, bm_open_depth: i32) {
        self.doc.pdoc.options.outline_open_depth = if bm_open_depth >= 0 {
            bm_open_depth
        } else {
            MAX_OUTLINE_DEPTH - bm_open_depth
        };
        self.doc.pdoc.outlines.current_depth = 1;
        self.doc.pdoc.outlines.items = vec![PdfOlitem {
            is_open: 1,
            ..PdfOlitem::default()
        }];
        self.doc.pdoc.outlines.current = Some(0);
        self.doc.pdoc.outlines.first = Some(0);
    }

    fn clean_bookmarks(&mut self, item: Option<usize>) {
        let mut item = item;
        while let Some(i) = item {
            let next = self.doc.pdoc.outlines.items[i].next;
            if let Some(d) = self.doc.pdoc.outlines.items[i].dict.take() {
                self.o.release(d);
            }
            let first = self.doc.pdoc.outlines.items[i].first;
            if first.is_some() {
                self.clean_bookmarks(first);
            }
            item = next;
        }
    }

    fn flush_bookmarks(&mut self, node: usize, parent_ref: Obj, parent_dict: Obj) -> i32 {
        let node_dict = self.doc.pdoc.outlines.items[node].dict.expect("bookmark");
        let mut this_ref = Some(self.o.ref_obj(node_dict));
        let l = self.o.link(this_ref.expect("ref"));
        self.o.put(parent_dict, b"First", l);
        let mut retval = 0;
        let mut prev_ref: Option<Obj> = None;
        let mut item = Some(node);
        while let Some(i) = item {
            let Some(dict) = self.doc.pdoc.outlines.items[i].dict else {
                break;
            };
            if let Some(f) = self.doc.pdoc.outlines.items[i].first
                && self.doc.pdoc.outlines.items[f].dict.is_some()
            {
                let count = self.flush_bookmarks(f, this_ref.expect("ref"), dict);
                if self.doc.pdoc.outlines.items[i].is_open != 0 {
                    self.o.put_number(dict, b"Count", f64::from(count));
                    retval += count;
                } else {
                    self.o.put_number(dict, b"Count", f64::from(-count));
                }
            }
            let p = self.o.link(parent_ref);
            self.o.put(dict, b"Parent", p);
            if let Some(pr) = prev_ref {
                self.o.put(dict, b"Prev", pr);
            }
            let next = self.doc.pdoc.outlines.items[i].next;
            let next_ref = if let Some(n) = next
                && let Some(nd) = self.doc.pdoc.outlines.items[n].dict
            {
                let r = self.o.ref_obj(nd);
                let l = self.o.link(r);
                self.o.put(dict, b"Next", l);
                Some(r)
            } else {
                None
            };
            self.o.release(dict);
            self.doc.pdoc.outlines.items[i].dict = None;
            prev_ref = this_ref;
            this_ref = next_ref;
            retval += 1;
            item = next;
        }
        let l = self.o.link_opt(prev_ref);
        self.o.put_opt(parent_dict, b"Last", l);
        self.o.release_opt(prev_ref);
        // (C releases node->dict again: it is NULL by now)
        retval
    }

    /// `pdf_doc_bookmarks_up`.
    pub fn pdf_doc_bookmarks_up(&mut self) -> i32 {
        let Some(item) = self.doc.pdoc.outlines.current else {
            return -1;
        };
        let Some(parent) = self.doc.pdoc.outlines.items[item].parent else {
            return -1;
        };
        let next = match self.doc.pdoc.outlines.items[parent].next {
            Some(n) => n,
            None => {
                let pp = self.doc.pdoc.outlines.items[parent].parent;
                self.doc.pdoc.outlines.items.push(PdfOlitem {
                    parent: pp,
                    ..PdfOlitem::default()
                });
                let n = self.doc.pdoc.outlines.items.len() - 1;
                self.doc.pdoc.outlines.items[parent].next = Some(n);
                n
            }
        };
        self.doc.pdoc.outlines.current = Some(next);
        self.doc.pdoc.outlines.current_depth -= 1;
        0
    }

    /// `pdf_doc_bookmarks_down`.
    pub fn pdf_doc_bookmarks_down(&mut self) -> i32 {
        let item = self.doc.pdoc.outlines.current.expect("bookmark");
        if self.doc.pdoc.outlines.items[item].dict.is_none() {
            let d = self.o.new_dict();
            self.o.put_string(d, b"Title", b"<No Title>");
            let tcolor = self.o.new_array();
            for v in [1.0, 0.0, 0.0] {
                let n = self.o.new_number(v);
                self.o.add_array(tcolor, n);
            }
            let l = self.o.link(tcolor);
            self.o.put(d, b"C", l);
            self.o.release(tcolor);
            self.o.put_number(d, b"F", 1.0);
            let action = self.o.new_dict();
            self.o.put_name(action, b"S", b"JavaScript");
            self.o.put_string(
                action,
                b"JS",
                b"app.alert(\"The author of this document made this bookmark item empty!\", 3, 0)",
            );
            let l = self.o.link(action);
            self.o.put(d, b"A", l);
            self.o.release(action);
            self.doc.pdoc.outlines.items[item].dict = Some(d);
        }
        self.doc.pdoc.outlines.items.push(PdfOlitem {
            parent: Some(item),
            ..PdfOlitem::default()
        });
        let first = self.doc.pdoc.outlines.items.len() - 1;
        self.doc.pdoc.outlines.items[item].first = Some(first);
        self.doc.pdoc.outlines.current = Some(first);
        self.doc.pdoc.outlines.current_depth += 1;
        0
    }

    /// `pdf_doc_bookmarks_depth`.
    pub fn pdf_doc_bookmarks_depth(&mut self) -> i32 {
        self.doc.pdoc.outlines.current_depth
    }

    /// `pdf_doc_bookmarks_add`: takes `dict`.
    pub fn pdf_doc_bookmarks_add(&mut self, dict: Obj, is_open: i32) -> Result<()> {
        let item = match self.doc.pdoc.outlines.current {
            None => {
                self.doc.pdoc.outlines.items.push(PdfOlitem::default());
                let i = self.doc.pdoc.outlines.items.len() - 1;
                self.doc.pdoc.outlines.first = Some(i);
                i
            }
            Some(c) if self.doc.pdoc.outlines.items[c].dict.is_some() => {
                self.doc.pdoc.outlines.items[c].next.expect("next")
            }
            Some(c) => c,
        };
        let open = if is_open < 0 {
            i32::from(
                self.doc.pdoc.outlines.current_depth <= self.doc.pdoc.options.outline_open_depth,
            )
        } else {
            is_open
        };
        let parent = self.doc.pdoc.outlines.items[item].parent;
        self.doc.pdoc.outlines.items.push(PdfOlitem {
            parent,
            is_open: -1,
            ..PdfOlitem::default()
        });
        let next = self.doc.pdoc.outlines.items.len() - 1;
        let it = &mut self.doc.pdoc.outlines.items[item];
        it.dict = Some(dict);
        it.first = None;
        it.is_open = open;
        it.next = Some(next);
        self.doc.pdoc.outlines.current = Some(item);
        self.pdf_doc_add_goto(dict)?;
        Ok(())
    }

    fn pdf_doc_close_bookmarks(&mut self) {
        let catalog = self.doc.pdoc.root.dict.expect("catalog");
        let item = self.doc.pdoc.outlines.first.expect("bookmarks");
        if self.doc.pdoc.outlines.items[item].dict.is_some() {
            let bm_root = self.o.new_dict();
            let bm_root_ref = self.o.ref_obj(bm_root);
            let count = self.flush_bookmarks(item, bm_root_ref, bm_root);
            self.o.put_number(bm_root, b"Count", f64::from(count));
            self.o.put(catalog, b"Outlines", bm_root_ref);
            self.o.release(bm_root);
        }
        self.clean_bookmarks(Some(item));
        self.doc.pdoc.outlines.items.clear();
        self.doc.pdoc.outlines.first = None;
        self.doc.pdoc.outlines.current = None;
        self.doc.pdoc.outlines.current_depth = 0;
    }

    fn pdf_doc_init_names(&mut self, check_gotos: i32) {
        self.doc.pdoc.root.names = None;
        self.doc.pdoc.names = NAME_DICT_CATEGORIES
            .iter()
            .map(|&c| NameDict {
                category: c,
                data: if c == b"Dests" {
                    Some(crate::pdfnames::pdf_new_name_tree())
                } else {
                    None
                },
            })
            .collect();
        self.doc.pdoc.check_gotos = check_gotos;
        self.doc.pdoc.gotos = HtTable::ht_init_table();
    }

    /// `pdf_doc_add_names`.
    pub fn pdf_doc_add_names(&mut self, category: &[u8], key: &[u8], value: Obj) -> i32 {
        let Some(i) = NAME_DICT_CATEGORIES.iter().position(|&c| c == category) else {
            return -1;
        };
        let tree = self.doc.pdoc.names[i]
            .data
            .get_or_insert_with(crate::pdfnames::pdf_new_name_tree);
        self.o.pdf_names_add_object(tree, key, value)
    }

    fn pdf_doc_add_goto(&mut self, annot_dict: Obj) -> Result<()> {
        if self.doc.pdoc.check_gotos == 0 {
            return Ok(());
        }
        let mut subtype = None;
        let mut a = None;
        let mut s = None;
        let mut d = None;
        // (labels as in C: cleanup, error, undefined all release and return)
        'body: {
            let st = self.o.lookup_dict(annot_dict, b"Subtype");
            subtype = self.o.deref_obj(st)?;
            if let Some(stv) = subtype {
                if self.o.is_undefined(Some(stv)) || !self.o.is_name(Some(stv)) {
                    break 'body;
                } else if self.o.name_value(stv) != b"Link" {
                    break 'body;
                }
            }
            let mut dict = annot_dict;
            let mut key: &[u8] = b"Dest";
            let dv = self.o.lookup_dict(annot_dict, key);
            d = self.o.deref_obj(dv)?;
            if self.o.is_undefined(d) {
                break 'body;
            }
            let av = self.o.lookup_dict(annot_dict, b"A");
            a = self.o.deref_obj(av)?;
            if let Some(av) = a {
                if self.o.is_undefined(Some(av)) || d.is_some() || !self.o.is_dict(Some(av)) {
                    break 'body;
                }
                let sv = self.o.lookup_dict(av, b"S");
                s = self.o.deref_obj(sv)?;
                if self.o.is_undefined(s) || !self.o.is_name(s) {
                    break 'body;
                } else if self.o.name_value(s.expect("name")) != b"GoTo" {
                    break 'body;
                }
                dict = av;
                key = b"D";
                let dv = self.o.lookup_dict(av, key);
                d = self.o.deref_obj(dv)?;
            }
            let dest: Vec<u8> = if self.o.is_string(d) {
                self.o.string_value(d.expect("string")).to_vec()
            } else {
                break 'body;
            };
            let d_new = if let Some(&x) = self.doc.pdoc.gotos.ht_lookup_table(&dest) {
                x
            } else {
                let mut b = crate::fmt::Buf::new();
                let _ = core::fmt::Write::write_fmt(
                    &mut b,
                    format_args!("{:x}", self.doc.pdoc.gotos.ht_table_size()),
                );
                let x = self.o.new_string(&b.0);
                self.doc.pdoc.gotos.ht_append_table(&dest, x);
                x
            };
            let key_obj = self.o.new_name(key);
            let l = self.o.link(d_new);
            // (C releases key_obj when the key is new: a leak-avoiding bug
            // that frees the key now held by the dictionary; the name is
            // still written, so the bytes are the same)
            self.o.add_dict(dict, key_obj, Some(l));
        }
        self.o.release_opt(subtype);
        self.o.release_opt(a);
        self.o.release_opt(s);
        self.o.release_opt(d);
        Ok(())
    }

    fn pdf_doc_close_names(&mut self) -> Result<()> {
        for i in 0..NAME_DICT_CATEGORIES.len() {
            let Some(mut data) = self.doc.pdoc.names[i].data.take() else {
                continue;
            };
            let use_filter = self.doc.pdoc.check_gotos != 0 && NAME_DICT_CATEGORIES[i] == b"Dests";
            let (tree, _count) = if use_filter {
                self.o
                    .pdf_names_create_tree(&mut data, Some(&self.doc.pdoc.gotos))?
            } else {
                self.o.pdf_names_create_tree(&mut data, None)?
            };
            if let Some(t) = tree {
                let names = match self.doc.pdoc.root.names {
                    Some(n) => n,
                    None => {
                        let n = self.o.new_dict();
                        self.doc.pdoc.root.names = Some(n);
                        n
                    }
                };
                let r = self.o.ref_obj(t);
                self.o.put(names, NAME_DICT_CATEGORIES[i], r);
                self.o.release(t);
            }
            self.o.pdf_delete_name_tree(data);
        }
        if let Some(names) = self.doc.pdoc.root.names.take() {
            let root = self.doc.pdoc.root.dict.expect("catalog");
            match self.o.lookup_dict(root, b"Names") {
                None => {
                    let r = self.o.ref_obj(names);
                    self.o.put(root, b"Names", r);
                }
                Some(t) if self.o.is_dict(Some(t)) => {
                    self.o.merge_dict(names, t);
                    let r = self.o.ref_obj(names);
                    self.o.put(root, b"Names", r);
                }
                Some(_) => {}
            }
            self.o.release(names);
        }
        self.doc.pdoc.names.clear();
        for v in self.doc.pdoc.gotos.ht_clear_table() {
            self.o.release(v);
        }
        Ok(())
    }

    /// `pdf_doc_add_annot`.
    pub fn pdf_doc_add_annot(
        &mut self,
        page_no: u32,
        rect: &PdfRect,
        annot_dict: Obj,
        new_annot: i32,
    ) -> Result<()> {
        let i = self.doc_get_page_entry(page_no);
        let annots = match self.doc.pdoc.pages.entries[i].annots {
            Some(a) => a,
            None => {
                let a = self.o.new_array();
                self.doc.pdoc.pages.entries[i].annots = Some(a);
                a
            }
        };
        let ra = self.o.new_array();
        for v in [rect.llx, rect.lly, rect.urx, rect.ury] {
            let n = self.o.new_number(round_acc(v, 0.001));
            self.o.add_array(ra, n);
        }
        self.o.put(annot_dict, b"Rect", ra);
        let r = self.o.ref_obj(annot_dict);
        self.o.add_array(annots, r);
        if new_annot != 0 {
            self.pdf_doc_add_goto(annot_dict)?;
        }
        Ok(())
    }

    fn pdf_doc_init_articles(&mut self) {
        self.doc.pdoc.root.threads = None;
        self.doc.pdoc.articles.clear();
    }

    /// `pdf_doc_begin_article`.
    pub fn pdf_doc_begin_article(&mut self, article_id: &[u8], article_info: Option<Obj>) {
        assert!(
            !article_id.is_empty(),
            "Article thread without internal identifier."
        );
        self.doc.pdoc.articles.push(PdfArticle {
            id: article_id.to_vec(),
            info: article_info,
            beads: Vec::new(),
        });
    }

    /// `pdf_doc_add_bead`.
    pub fn pdf_doc_add_bead(
        &mut self,
        article_id: &[u8],
        bead_id: Option<&[u8]>,
        page_no: i32,
        rect: &PdfRect,
    ) {
        let a = self
            .doc
            .pdoc
            .articles
            .iter()
            .position(|a| a.id == article_id)
            .expect("Specified article thread that doesn't exist.");
        let article = &mut self.doc.pdoc.articles[a];
        let found = bead_id.and_then(|b| {
            article
                .beads
                .iter()
                .position(|x| x.id.as_deref() == Some(b))
        });
        let k = match found {
            Some(k) => k,
            None => {
                article.beads.push(PdfBead {
                    id: bead_id.map(<[u8]>::to_vec),
                    page_no: -1,
                    rect: PdfRect::default(),
                });
                article.beads.len() - 1
            }
        };
        article.beads[k].rect = *rect;
        article.beads[k].page_no = page_no;
    }

    fn make_article(&mut self, a: usize) -> Option<Obj> {
        let art_dict = self.o.new_dict();
        let mut first: Option<Obj> = None;
        let mut prev: Option<Obj> = None;
        let mut last: Option<Obj> = None;
        let n = self.doc.pdoc.articles[a].beads.len();
        for i in 0..n {
            let bead = self.doc.pdoc.articles[a].beads[i].clone();
            if bead.page_no < 0 {
                continue;
            }
            let l = self.o.new_dict();
            last = Some(l);
            if let Some(p) = prev {
                let r = self.o.ref_obj(l);
                self.o.put(p, b"N", r);
                let r = self.o.ref_obj(p);
                self.o.put(l, b"V", r);
                if Some(p) != first {
                    self.o.release(p);
                }
            } else {
                first = Some(l);
                let r = self.o.ref_obj(art_dict);
                self.o.put(l, b"T", r);
            }
            let pi = self.doc_get_page_entry(bead.page_no as u32);
            let beads = match self.doc.pdoc.pages.entries[pi].beads {
                Some(b) => b,
                None => {
                    let b = self.o.new_array();
                    self.doc.pdoc.pages.entries[pi].beads = Some(b);
                    b
                }
            };
            let pr = self.doc.pdoc.pages.entries[pi].page_ref;
            let pl = self.o.link_opt(pr);
            self.o.put_opt(l, b"P", pl);
            let rect = self.o.new_array();
            for v in [bead.rect.llx, bead.rect.lly, bead.rect.urx, bead.rect.ury] {
                let x = self.o.new_number(round_acc(v, 0.01));
                self.o.add_array(rect, x);
            }
            self.o.put(l, b"R", rect);
            let r = self.o.ref_obj(l);
            self.o.add_array(beads, r);
            prev = Some(l);
        }
        if let (Some(f), Some(l)) = (first, last) {
            let r = self.o.ref_obj(f);
            self.o.put(l, b"N", r);
            let r = self.o.ref_obj(l);
            self.o.put(f, b"V", r);
            if f != l {
                self.o.release(l);
            }
            let r = self.o.ref_obj(f);
            self.o.put(art_dict, b"F", r);
            if let Some(info) = self.doc.pdoc.articles[a].info.take() {
                let r = self.o.ref_obj(info);
                self.o.put(art_dict, b"I", r);
                self.o.release(info);
            }
            self.o.release(f);
            Some(art_dict)
        } else {
            self.o.release(art_dict);
            None
        }
    }

    fn pdf_doc_close_articles(&mut self) {
        for a in 0..self.doc.pdoc.articles.len() {
            if !self.doc.pdoc.articles[a].beads.is_empty() {
                let art = self.make_article(a);
                let threads = match self.doc.pdoc.root.threads {
                    Some(t) => t,
                    None => {
                        let t = self.o.new_array();
                        self.doc.pdoc.root.threads = Some(t);
                        t
                    }
                };
                // (C refers to a null article dict: `pdf_ref_obj(NULL)` is an
                // error; never with beads)
                let art = art.expect("article");
                let r = self.o.ref_obj(art);
                self.o.add_array(threads, r);
                self.o.release(art);
            }
        }
        self.doc.pdoc.articles.clear();
        if let Some(t) = self.doc.pdoc.root.threads.take() {
            let root = self.doc.pdoc.root.dict.expect("catalog");
            let r = self.o.ref_obj(t);
            self.o.put(root, b"Threads", r);
            self.o.release(t);
        }
    }

    /// `pdf_doc_set_mediabox`: 0 for the root.
    pub fn pdf_doc_set_mediabox(&mut self, page_no: u32, mediabox: &PdfRect) {
        if page_no == 0 {
            self.doc.pdoc.pages.mediabox = *mediabox;
        } else {
            let i = self.doc_get_page_entry(page_no);
            self.doc.pdoc.pages.entries[i].cropbox = *mediabox;
            self.doc.pdoc.pages.entries[i].flags |= USE_MY_MEDIABOX;
        }
    }

    /// `pdf_doc_get_mediabox`.
    pub fn pdf_doc_get_mediabox(&mut self, page_no: u32) -> PdfRect {
        if page_no == 0 {
            self.doc.pdoc.pages.mediabox
        } else {
            let i = self.doc_get_page_entry(page_no);
            if self.doc.pdoc.pages.entries[i].flags & USE_MY_MEDIABOX != 0 {
                self.doc.pdoc.pages.entries[i].cropbox
            } else {
                self.doc.pdoc.pages.mediabox
            }
        }
    }

    /// `pdf_doc_current_page_resources`.
    pub fn pdf_doc_current_page_resources(&mut self) -> Option<Obj> {
        Some(self.doc_current_page_resources())
    }

    fn doc_current_page_resources(&mut self) -> Obj {
        if let Some(f) = self.doc.pdoc.pending_forms.last() {
            if let Some(r) = f.form.resources {
                r
            } else {
                let r = self.o.new_dict();
                self.doc
                    .pdoc
                    .pending_forms
                    .last_mut()
                    .expect("form")
                    .form
                    .resources = Some(r);
                r
            }
        } else {
            let i = self.doc.pdoc.pages.num_entries;
            self.doc_resize_page_entries(i + 1);
            if let Some(r) = self.doc.pdoc.pages.entries[i].resources {
                r
            } else {
                let r = self.o.new_dict();
                self.doc.pdoc.pages.entries[i].resources = Some(r);
                r
            }
        }
    }

    /// `pdf_doc_get_dictionary`.
    pub fn pdf_doc_get_dictionary(&mut self, category: &[u8]) -> Obj {
        let d = match category {
            b"Names" => {
                if self.doc.pdoc.root.names.is_none() {
                    self.doc.pdoc.root.names = Some(self.o.new_dict());
                }
                self.doc.pdoc.root.names
            }
            b"Pages" => {
                if self.doc.pdoc.root.pages.is_none() {
                    self.doc.pdoc.root.pages = Some(self.o.new_dict());
                }
                self.doc.pdoc.root.pages
            }
            b"Catalog" => {
                if self.doc.pdoc.root.dict.is_none() {
                    self.doc.pdoc.root.dict = Some(self.o.new_dict());
                }
                self.doc.pdoc.root.dict
            }
            b"Info" => {
                if self.doc.pdoc.info.is_none() {
                    self.doc.pdoc.info = Some(self.o.new_dict());
                }
                self.doc.pdoc.info
            }
            b"@THISPAGE" => {
                let i = self.doc.pdoc.pages.num_entries;
                self.doc_resize_page_entries(i + 1);
                self.doc.pdoc.pages.entries[i].page_obj
            }
            _ => None,
        };
        d.expect("Document dict. not exist.")
    }

    /// `pdf_doc_current_page_number`.
    pub fn pdf_doc_current_page_number(&mut self) -> i32 {
        (self.doc.pdoc.pages.num_entries + 1) as i32
    }

    /// `pdf_doc_ref_page`.
    pub fn pdf_doc_ref_page(&mut self, page_no: u32) -> Option<Obj> {
        let i = self.doc_get_page_entry(page_no);
        if self.doc.pdoc.pages.entries[i].page_obj.is_none() {
            let po = self.o.new_dict();
            self.doc.pdoc.pages.entries[i].page_obj = Some(po);
            self.doc.pdoc.pages.entries[i].page_ref = Some(self.o.ref_obj(po));
        }
        let r = self.doc.pdoc.pages.entries[i].page_ref.expect("page ref");
        Some(self.o.link(r))
    }

    /// `pdf_doc_get_reference`.
    pub fn pdf_doc_get_reference(&mut self, category: &[u8]) -> Option<Obj> {
        let page_no = self.pdf_doc_current_page_number() as u32;
        match category {
            b"@THISPAGE" => self.pdf_doc_ref_page(page_no),
            b"@PREVPAGE" => {
                assert!(
                    page_no > 1,
                    "Reference to previous page, but no pages have been completed yet."
                );
                self.pdf_doc_ref_page(page_no - 1)
            }
            b"@NEXTPAGE" => self.pdf_doc_ref_page(page_no + 1),
            _ => panic!("Reference to category not exist."),
        }
    }

    fn pdf_doc_new_page(&mut self) {
        let n = self.doc.pdoc.pages.num_entries;
        if n >= self.doc.pdoc.pages.entries.len() {
            let m = self.doc.pdoc.pages.entries.len() + PDFDOC_PAGES_ALLOC_SIZE;
            self.doc_resize_page_entries(m);
        }
        if self.doc.pdoc.pages.entries[n].page_ref.is_none() {
            let po = self.o.new_dict();
            self.doc.pdoc.pages.entries[n].page_obj = Some(po);
            self.doc.pdoc.pages.entries[n].page_ref = Some(self.o.ref_obj(po));
        }
        self.doc.pdoc.pages.entries[n].background = None;
        self.doc.pdoc.pages.entries[n].contents = Some(self.o.new_stream(STREAM_COMPRESS));
        self.doc.pdoc.pages.entries[n].resources = Some(self.o.new_dict());
        self.doc.pdoc.pages.entries[n].annots = None;
        self.doc.pdoc.pages.entries[n].beads = None;
    }

    fn pdf_doc_finish_page(&mut self) {
        assert!(
            self.doc.pdoc.pending_forms.is_empty(),
            "A pending form XObject at the end of page."
        );
        let n = self.doc.pdoc.pages.num_entries;
        if self.doc.pdoc.pages.entries[n].page_obj.is_none() {
            self.doc.pdoc.pages.entries[n].page_obj = Some(self.o.new_dict());
        }
        self.doc.pdoc.pages.entries[n].content_refs[0] = match self.doc.pdoc.pages.bop {
            Some(b) if self.o.stream_length(b) > 0 => Some(self.o.ref_obj(b)),
            _ => None,
        };
        if let Some(bg) = self.doc.pdoc.pages.entries[n].background.take() {
            if self.o.stream_length(bg) > 0 {
                self.doc.pdoc.pages.entries[n].content_refs[1] = Some(self.o.ref_obj(bg));
                self.o.add_stream(bg, b"\n");
            }
            self.o.release(bg);
        } else {
            self.doc.pdoc.pages.entries[n].content_refs[1] = None;
        }
        let contents = self.doc.pdoc.pages.entries[n]
            .contents
            .take()
            .expect("contents");
        self.doc.pdoc.pages.entries[n].content_refs[2] = Some(self.o.ref_obj(contents));
        self.o.add_stream(contents, b"\n");
        self.o.release(contents);
        self.doc.pdoc.pages.entries[n].content_refs[3] = match self.doc.pdoc.pages.eop {
            Some(e) if self.o.stream_length(e) > 0 => Some(self.o.ref_obj(e)),
            _ => None,
        };
        if let Some(res) = self.doc.pdoc.pages.entries[n].resources.take() {
            let procset = self.o.new_array();
            for name in [&b"PDF"[..], b"Text", b"ImageC", b"ImageB", b"ImageI"] {
                let x = self.o.new_name(name);
                self.o.add_array(procset, x);
            }
            self.o.put(res, b"ProcSet", procset);
            let po = self.doc.pdoc.pages.entries[n].page_obj.expect("page");
            let r = self.o.ref_obj(res);
            self.o.put(po, b"Resources", r);
            self.o.release(res);
        }
        // (manual thumbnails: not supported)
        self.doc.pdoc.pages.num_entries += 1;
    }

    /// `pdf_doc_set_bgcolor`.
    pub fn pdf_doc_set_bgcolor(&mut self, color: Option<&PdfColor>) {
        match color {
            Some(c) => self.doc.bgcolor = c.clone(),
            None => {
                let mut c = PdfColor::default();
                c.pdf_color_graycolor(1.0);
                self.doc.bgcolor = c;
            }
        }
    }

    fn doc_fill_page_background(&mut self) -> Result<()> {
        let cm = self.pdf_dev_get_param(crate::pdfdev::PDF_DEV_PARAM_COLORMODE);
        if cm == 0 || self.doc.bgcolor.pdf_color_is_white() != 0 {
            return Ok(());
        }
        let page_no = self.pdf_doc_current_page_number();
        let r = self.pdf_doc_get_mediabox(page_no as u32);
        let n = self.doc.pdoc.pages.num_entries;
        if self.doc.pdoc.pages.entries[n].background.is_none() {
            self.doc.pdoc.pages.entries[n].background = Some(self.o.new_stream(STREAM_COMPRESS));
        }
        let saved = self.doc.pdoc.pages.entries[n].contents;
        self.doc.pdoc.pages.entries[n].contents = self.doc.pdoc.pages.entries[n].background;
        self.pdf_dev_gsave();
        let bg = self.doc.bgcolor.clone();
        self.pdf_dev_set_nonstrokingcolor(&bg)?;
        self.pdf_dev_rectfill(r.llx, r.lly, r.urx - r.llx, r.ury - r.lly);
        self.pdf_dev_grestore();
        self.doc.pdoc.pages.entries[n].contents = saved;
        Ok(())
    }

    /// `pdf_doc_begin_page`.
    pub fn pdf_doc_begin_page(&mut self, scale: f64, x_origin: f64, y_origin: f64) -> Result<()> {
        let m = PdfTmatrix {
            a: scale,
            b: 0.0,
            c: 0.0,
            d: scale,
            e: x_origin,
            f: y_origin,
        };
        self.pdf_doc_new_page();
        self.pdf_dev_bop(&m)?;
        Ok(())
    }

    /// `pdf_doc_end_page`.
    pub fn pdf_doc_end_page(&mut self) -> Result<()> {
        self.pdf_dev_eop();
        self.doc_fill_page_background()?;
        self.pdf_doc_finish_page();
        Ok(())
    }

    /// `pdf_doc_add_page_content`.
    pub fn pdf_doc_add_page_content(&mut self, buffer: &[u8]) {
        let s = if let Some(f) = self.doc.pdoc.pending_forms.last() {
            f.form.contents
        } else {
            self.doc.pdoc.pages.entries[self.doc.pdoc.pages.num_entries].contents
        };
        if let Some(s) = s {
            self.o.add_stream(s, buffer);
        }
    }

    /// `pdf_open_document`.
    pub fn pdf_open_document(
        &mut self,
        filename: Option<&[u8]>,
        creator: Option<&[u8]>,
        id1: &[u8; 16],
        id2: &[u8; 16],
        settings: PdfSetting,
    ) -> Result<()> {
        let s = &settings;
        let _ = filename; // (only the thumbnails' base name, not ported)
        self.o.init(
            id1,
            id2,
            s.ver_major,
            s.ver_minor,
            s.object.compression_level,
            s.object.enable_objstm != 0,
            s.object.enable_predictor != 0,
        );
        self.pdf_doc_init_catalog();
        self.doc.pdoc.options.annot_grow.0 = s.annot_grow_amount.0;
        self.doc.pdoc.options.annot_grow.1 = s.annot_grow_amount.1;
        self.doc.pdoc.options.outline_open_depth = s.outline_open_depth;
        self.pdf_init_resources();
        self.pdf_init_colors();
        self.pdf_init_fonts()?;
        self.pdf_init_images();
        self.pdf_doc_init_docinfo();
        if let Some(c) = creator {
            let info = self.doc.pdoc.info.expect("info");
            self.o.put_string(info, b"Creator", c);
        }
        self.pdf_doc_init_bookmarks(s.outline_open_depth);
        self.pdf_doc_init_articles();
        self.pdf_doc_init_names(s.check_gotos);
        self.pdf_doc_init_page_tree(s.media_width, s.media_height);
        self.pdf_doc_set_bgcolor(None);
        self.doc.pdoc.options.enable_manual_thumb = s.enable_manual_thumb;
        self.doc.pdoc.pending_forms.clear();
        self.pdf_init_device(s.device.dvi2pts, s.device.precision, s.device.ignore_colors);
        self.doc.global_names = Some(crate::pdfnames::pdf_new_name_tree());
        Ok(())
    }

    /// `pdf_close_document`.
    pub fn pdf_close_document(&mut self) -> Result<()> {
        if let Some(g) = self.doc.global_names.take() {
            self.o.pdf_delete_name_tree(g);
        }
        self.pdf_close_device();
        self.pdf_doc_close_articles();
        self.pdf_doc_close_names()?;
        self.pdf_doc_close_bookmarks();
        self.pdf_doc_close_page_tree();
        self.pdf_doc_close_docinfo();
        self.pdf_doc_close_catalog();
        self.pdf_close_images();
        self.pdf_close_fonts()?;
        self.pdf_close_colors();
        self.pdf_close_resources();
        self.o.flush();
        Ok(())
    }

    fn pdf_doc_make_xform(
        &mut self,
        xform: Obj,
        bbox: &PdfRect,
        matrix: Option<&PdfTmatrix>,
        resources: Obj,
        attrib: Option<Obj>,
    ) {
        let d = self.o.stream_dict(xform);
        self.o.put_name(d, b"Type", b"XObject");
        self.o.put_name(d, b"Subtype", b"Form");
        self.o.put_number(d, b"FormType", 1.0);
        let t = self.o.new_array();
        for v in [bbox.llx, bbox.lly, bbox.urx, bbox.ury] {
            let n = self.o.new_number(round_acc(v, 0.001));
            self.o.add_array(t, n);
        }
        self.o.put(d, b"BBox", t);
        if let Some(m) = matrix {
            let t = self.o.new_array();
            for (v, acc) in [
                (m.a, 0.00001),
                (m.b, 0.00001),
                (m.c, 0.00001),
                (m.d, 0.00001),
                (m.e, 0.001),
                (m.f, 0.001),
            ] {
                let n = self.o.new_number(round_acc(v, acc));
                self.o.add_array(t, n);
            }
            self.o.put(d, b"Matrix", t);
        }
        if let Some(a) = attrib {
            self.o.merge_dict(d, a);
        }
        self.o.put(d, b"Resources", resources);
    }

    /// `pdf_doc_begin_grabbing`: the form's XObject id.
    pub fn pdf_doc_begin_grabbing(
        &mut self,
        ident: &[u8],
        ref_x: f64,
        ref_y: f64,
        cropbox: &PdfRect,
    ) -> Result<i32> {
        self.pdf_dev_push_gstate();
        let q_depth = self.pdf_dev_current_depth();
        let contents = self.o.new_stream(STREAM_COMPRESS);
        let resources = self.o.new_dict();
        let node = FormListNode {
            q_depth,
            form: PdfForm {
                ident: ident.to_vec(),
                matrix: PdfTmatrix {
                    a: 1.0,
                    b: 0.0,
                    c: 0.0,
                    d: 1.0,
                    e: -ref_x,
                    f: -ref_y,
                },
                cropbox: PdfRect {
                    llx: ref_x + cropbox.llx,
                    lly: ref_y + cropbox.lly,
                    urx: ref_x + cropbox.urx,
                    ury: ref_y + cropbox.ury,
                },
                resources: Some(resources),
                contents: Some(contents),
            },
        };
        let mut info = crate::pdfximage::pdf_ximage_init_form_info();
        info.matrix = PdfTmatrix {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: -ref_x,
            f: -ref_y,
        };
        info.bbox = *cropbox;
        let l = self.o.link(contents);
        let xobj_id = self.pdf_ximage_defineresource(
            Some(ident),
            crate::pdfximage::PDF_XOBJECT_TYPE_FORM,
            &crate::pdfximage::XobjInfo::Form(info),
            l,
        )?;
        self.doc.pdoc.pending_forms.push(node);
        self.pdf_dev_reset_fonts(1);
        self.pdf_dev_reset_color(1)?;
        self.pdf_dev_reset_xgstate(1)?;
        Ok(xobj_id)
    }

    /// `pdf_doc_end_grabbing`: takes `attrib`.
    pub fn pdf_doc_end_grabbing(&mut self, attrib: Option<Obj>) -> Result<()> {
        let Some(node) = self.doc.pdoc.pending_forms.last().cloned() else {
            return Ok(());
        };
        self.pdf_dev_grestore_to(node.q_depth);
        let res = node.form.resources.expect("resources");
        let procset = self.o.new_array();
        for name in [&b"PDF"[..], b"Text", b"ImageC", b"ImageB", b"ImageI"] {
            let x = self.o.new_name(name);
            self.o.add_array(procset, x);
        }
        self.o.put(res, b"ProcSet", procset);
        let contents = node.form.contents.expect("contents");
        let r = self.o.ref_obj(res);
        self.pdf_doc_make_xform(
            contents,
            &node.form.cropbox,
            Some(&node.form.matrix),
            r,
            attrib,
        );
        self.o.release(res);
        self.o.release(contents);
        self.o.release_opt(attrib);
        self.doc.pdoc.pending_forms.pop();
        self.pdf_dev_pop_gstate();
        self.pdf_dev_reset_fonts(1);
        self.pdf_dev_reset_color(0)?;
        self.pdf_dev_reset_xgstate(0)?;
        Ok(())
    }

    fn reset_box(&mut self) {
        let b = &mut self.doc.breaking_state;
        b.rect = PdfRect {
            llx: f64::INFINITY,
            lly: f64::INFINITY,
            urx: f64::NEG_INFINITY,
            ury: f64::NEG_INFINITY,
        };
        b.dirty = 0;
    }

    /// `pdf_doc_begin_annot`: takes `dict`.
    pub fn pdf_doc_begin_annot(&mut self, dict: Obj) {
        self.doc.breaking_state.annot_dict = Some(dict);
        self.doc.breaking_state.broken = 0;
        self.reset_box();
    }

    /// `pdf_doc_end_annot`.
    pub fn pdf_doc_end_annot(&mut self) -> Result<()> {
        self.pdf_doc_break_annot()?;
        if let Some(d) = self.doc.breaking_state.annot_dict.take() {
            self.o.release(d);
        }
        Ok(())
    }

    /// `pdf_doc_break_annot`.
    pub fn pdf_doc_break_annot(&mut self) -> Result<()> {
        if self.doc.breaking_state.dirty != 0 {
            let annot_dict = self.doc.breaking_state.annot_dict.expect("annotation");
            let copy = self.o.new_dict();
            self.o.merge_dict(copy, annot_dict);
            self.doc.breaking_state.annot_dict = Some(copy);
            let mut rect = self.doc.breaking_state.rect;
            rect.llx -= self.doc.pdoc.options.annot_grow.0;
            rect.lly -= self.doc.pdoc.options.annot_grow.1;
            rect.urx += self.doc.pdoc.options.annot_grow.0;
            rect.ury += self.doc.pdoc.options.annot_grow.1;
            let page = self.pdf_doc_current_page_number();
            let broken = self.doc.breaking_state.broken;
            self.pdf_doc_add_annot(page as u32, &rect, annot_dict, i32::from(broken == 0))?;
            self.o.release(annot_dict);
            self.doc.breaking_state.broken = 1;
        }
        self.reset_box();
        Ok(())
    }

    /// `pdf_doc_expand_box`.
    pub fn pdf_doc_expand_box(&mut self, rect: &PdfRect) {
        let b = &mut self.doc.breaking_state;
        b.rect.llx = b.rect.llx.min(rect.llx);
        b.rect.lly = b.rect.lly.min(rect.lly);
        b.rect.urx = b.rect.urx.max(rect.urx);
        b.rect.ury = b.rect.ury.max(rect.ury);
        b.dirty = 1;
    }

    /// `pdf_doc_get_page_count`.
    pub fn pdf_doc_get_page_count(&mut self, pf: u32) -> Result<i32> {
        let catalog = self.o.pdf_file_catalog(pf);
        let p = catalog.and_then(|c| self.o.lookup_dict(c, b"Pages"));
        let page_tree = self.o.deref_obj(p)?;
        if !self.o.is_dict(page_tree) {
            return Ok(0);
        }
        let c = self.o.lookup_dict(page_tree.expect("dict"), b"Count");
        let tmp = self.o.deref_obj(c)?;
        if !self.o.is_number(tmp) {
            self.o.release_opt(tmp);
            return Ok(0);
        }
        let count = self.o.number_value(tmp.expect("number")) as i32;
        self.o.release_opt(tmp);
        Ok(count)
    }
}

/// Whether `a` is an array of 4 (`VALIDATE_BOX`).
fn valid_box(o: &crate::obj::PdfOut, b: Option<Obj>) -> bool {
    match b {
        Some(x) => o.type_of(Some(x)) == PDF_ARRAY && o.array_length(x) == 4,
        None => true,
    }
}

#[allow(dead_code)]
fn _unused(_: &[Option<Obj>]) {
    let _ = valid_box;
}

impl Dpx {
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

    /// `set_bounding_box`: status (0 ok) and the box.
    fn set_bounding_box(
        &mut self,
        opt_bbox: PdfPageBoundary,
        media_box: Option<Obj>,
        crop_box: Option<Obj>,
        art_box: Option<Obj>,
        trim_box: Option<Obj>,
        bleed_box: Option<Obj>,
    ) -> Result<(i32, PdfRect)> {
        let mut bbox = PdfRect::default();
        let Some(media_box) = media_box else {
            return Ok((-1, bbox));
        };
        for b in [Some(media_box), crop_box, art_box, trim_box, bleed_box]
            .into_iter()
            .flatten()
        {
            if !self.o.is_array(Some(b)) || self.o.array_length(b) != 4 {
                return Ok((-1, bbox));
            }
        }
        let boxo = if opt_bbox == PdfPageBoundary::Auto {
            let b = crop_box
                .or(art_box)
                .or(trim_box)
                .or(bleed_box)
                .unwrap_or(media_box);
            self.o.link(b)
        } else {
            // (C links the missing boxes from the others and never
            // releases them: the leaked counts are a file's objects,
            // never written)
            let crop_box = match crop_box {
                Some(c) => c,
                None => self.o.link(media_box),
            };
            let art_box = match art_box {
                Some(c) => c,
                None => self.o.link(crop_box),
            };
            let trim_box = match trim_box {
                Some(c) => c,
                None => self.o.link(crop_box),
            };
            let bleed_box = match bleed_box {
                Some(c) => c,
                None => self.o.link(crop_box),
            };
            let b = match opt_bbox {
                PdfPageBoundary::CropBox => crop_box,
                PdfPageBoundary::MediaBox => media_box,
                PdfPageBoundary::ArtBox => art_box,
                PdfPageBoundary::TrimBox => trim_box,
                PdfPageBoundary::BleedBox => bleed_box,
                PdfPageBoundary::Auto => crop_box,
            };
            self.o.link(b)
        };
        for i in (0..4).rev() {
            let e = self.o.get_array(boxo, i);
            let tmp = self.o.deref_obj(e)?;
            if !self.o.is_number(tmp) {
                self.o.release_opt(tmp);
                self.o.release(boxo);
                return Ok((-1, bbox));
            }
            let x = self.o.number_value(tmp.expect("number"));
            match i {
                0 => bbox.llx = x,
                1 => bbox.lly = x,
                2 => bbox.urx = x,
                _ => bbox.ury = x,
            }
            self.o.release_opt(tmp);
        }
        if self.conf.compat_mode == crate::ctx::CompatMode::Xdv || opt_bbox != PdfPageBoundary::Auto
        {
            for i in (0..4).rev() {
                let e = self.o.get_array(media_box, i);
                let tmp = self.o.deref_obj(e)?;
                if !self.o.is_number(tmp) {
                    self.o.release_opt(tmp);
                    self.o.release(boxo);
                    return Ok((-1, bbox));
                }
                let x = self.o.number_value(tmp.expect("number"));
                match i {
                    0 => {
                        if bbox.llx < x {
                            bbox.llx = x;
                        }
                    }
                    1 => {
                        if bbox.lly < x {
                            bbox.lly = x;
                        }
                    }
                    2 => {
                        if bbox.urx > x {
                            bbox.urx = x;
                        }
                    }
                    _ => {
                        if bbox.ury > x {
                            bbox.ury = x;
                        }
                    }
                }
                self.o.release_opt(tmp);
            }
        }
        self.o.release(boxo);
        Ok((0, bbox))
    }

    /// `set_transform_matrix`: status (0 ok) and the matrix.
    fn set_transform_matrix(
        &mut self,
        bbox: &mut PdfRect,
        rotate: Option<Obj>,
    ) -> (i32, PdfTmatrix) {
        let mut m = PdfTmatrix {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: 0.0,
            f: 0.0,
        };
        if let Some(r) = rotate {
            if !self.o.is_number(Some(r)) {
                return (-1, m);
            }
            let deg = self.o.number_value(r);
            if deg - f64::from(deg as i32) != 0.0 {
                return (-1, m);
            } else if deg != 0.0 {
                let mut rot = deg as i32;
                if rot % 90 == 0 {
                    rot %= 360;
                    if rot < 0 {
                        rot += 360;
                    }
                    match rot {
                        90 => {
                            m.a = 0.0;
                            m.d = 0.0;
                            m.b = -1.0;
                            m.c = 1.0;
                            m.e = bbox.llx - bbox.lly;
                            m.f = bbox.lly + bbox.urx;
                        }
                        180 => {
                            m.a = -1.0;
                            m.d = -1.0;
                            m.b = 0.0;
                            m.c = 0.0;
                            m.e = bbox.llx + bbox.urx;
                            m.f = bbox.lly + bbox.ury;
                        }
                        270 => {
                            m.a = 0.0;
                            m.d = 0.0;
                            m.b = 1.0;
                            m.c = -1.0;
                            m.e = bbox.llx + bbox.ury;
                            m.f = bbox.lly - bbox.llx;
                        }
                        _ => {}
                    }
                }
            }
        }
        (0, m)
    }

    /// `get_page_properties`.
    fn get_page_properties(&mut self, boxes: &mut PdfBoxes) -> Result<()> {
        let pt = boxes.page_tree.expect("page tree");
        for (key, slot) in [
            (&b"MediaBox"[..], &mut boxes.media_box),
            (b"CropBox", &mut boxes.crop_box),
            (b"ArtBox", &mut boxes.art_box),
            (b"TrimBox", &mut boxes.trim_box),
            (b"BleedBox", &mut boxes.bleed_box),
            (b"Rotate", &mut boxes.rotate),
            (b"Resources", &mut boxes.resources),
        ] {
            let v = self.o.lookup_dict(pt, key);
            if let Some(tmp) = self.o.deref_obj(v)? {
                let old = slot.replace(tmp);
                self.o.release_opt(old);
            }
        }
        Ok(())
    }

    /// `page_by_name`.
    fn page_by_name(&mut self, catalog: Obj, page_name: &[u8], boxes: &mut PdfBoxes) -> Result<()> {
        let le = |a: &[u8], b: &[u8]| strcmp_c(a, b);
        let v = self.o.lookup_dict(catalog, b"Names");
        let names0 = self.o.deref_obj(v)?;
        if !self.o.is_dict(names0) {
            self.o.release_opt(names0);
            return Ok(());
        }
        let v = self.o.lookup_dict(names0.expect("dict"), b"Dests");
        let mut dests = self.o.deref_obj(v)?;
        self.o.release_opt(names0);
        if !self.o.is_dict(dests) {
            self.o.release_opt(dests);
            return Ok(());
        }
        let mut pos = [0usize; 5];
        let mut level: i32 = 0;
        let mut up_dests: Option<Obj> = None;
        let mut found_names = None;
        let mut i = 0;
        while i < 1000 {
            if level < 0 || level as usize >= pos.len() {
                return Ok(());
            }
            let recurse = if i == 0 {
                true
            } else {
                let Some(d) = dests else { return Ok(()) };
                let v = self.o.lookup_dict(d, b"Limits");
                let limits = self.o.deref_obj(v)?;
                if !self.o.is_array(limits) {
                    self.o.release_opt(limits);
                    return Ok(());
                }
                let l = limits.expect("array");
                let e0 = self.o.get_array(l, 0);
                let start_obj = self.o.deref_obj(e0)?;
                let e1 = self.o.get_array(l, 1);
                let end_obj = self.o.deref_obj(e1)?;
                self.o.release(l);
                if !self.o.is_string(start_obj) || !self.o.is_string(end_obj) {
                    self.o.release_opt(start_obj);
                    self.o.release_opt(end_obj);
                    return Ok(());
                }
                let start = self.o.string_value(start_obj.expect("string")).to_vec();
                let end = self.o.string_value(end_obj.expect("string")).to_vec();
                self.o.release_opt(start_obj);
                self.o.release_opt(end_obj);
                le(page_name, &start) >= 0 && le(page_name, &end) <= 0
            };
            if recurse {
                let d = dests.expect("dests");
                let v = self.o.lookup_dict(d, b"Names");
                let names = self.o.deref_obj(v)?;
                if self.o.is_array(names) {
                    found_names = names;
                    break;
                }
                self.o.release_opt(names);
                let v = self.o.lookup_dict(d, b"Kids");
                let kids = self.o.deref_obj(v)?;
                if !self.o.is_array(kids) {
                    self.o.release_opt(kids);
                    return Ok(());
                }
                let kids = kids.expect("array");
                let kids_length = self.o.array_length(kids);
                if pos[level as usize] < kids_length {
                    up_dests = dests;
                    let e = self.o.get_array(kids, pos[level as usize] as i32);
                    dests = self.o.deref_obj(e)?;
                    level += 1;
                }
                // (C leaks `kids`)
            } else {
                level -= 1;
                if level >= 0 {
                    pos[level as usize] += 1;
                }
                self.o.release_opt(dests);
                dests = up_dests;
            }
            i += 1;
        }
        let Some(names) = found_names else {
            return Ok(());
        };
        let names_length = self.o.array_length(names);
        let mut page = None;
        let mut i = 0;
        while i < names_length {
            let e = self.o.get_array(names, i as i32);
            let name_obj = self.o.deref_obj(e)?;
            if !self.o.is_string(name_obj) {
                self.o.release_opt(name_obj);
                i += 1;
                continue;
            }
            let name = self.o.string_value(name_obj.expect("string")).to_vec();
            self.o.release_opt(name_obj);
            if strcmp_c(page_name, &name) != 0 {
                i += 1;
                continue;
            }
            i += 1;
            let e = self.o.get_array(names, i as i32);
            let dest = self.o.deref_obj(e)?;
            let dest_inner = if self.o.is_dict(dest) {
                let v = self.o.lookup_dict(dest.expect("dict"), b"D");
                let di = self.o.deref_obj(v)?;
                self.o.release_opt(dest);
                if !self.o.is_array(di) {
                    self.o.release_opt(di);
                    return Ok(());
                }
                di
            } else if self.o.is_array(dest) {
                dest
            } else {
                self.o.release_opt(dest);
                i += 1;
                continue;
            };
            let e = self.o.get_array(dest_inner.expect("array"), 0);
            page = self.o.deref_obj(e)?;
            self.o.release_opt(dest_inner);
            if !self.o.is_dict(page) {
                self.o.release_opt(page);
                return Ok(());
            }
            break;
        }
        boxes.page_tree = page;
        loop {
            // (C's boxes->page_tree is NULL here when the name was not in
            // the array: get_page_properties would crash)
            if boxes.page_tree.is_none() {
                return Ok(());
            }
            self.get_page_properties(boxes)?;
            if boxes.media_box.is_some() {
                break;
            }
            let v = self
                .o
                .lookup_dict(boxes.page_tree.expect("dict"), b"Parent");
            boxes.page_tree = self.o.deref_obj(v)?;
            if !self.o.is_dict(boxes.page_tree) {
                self.o.release_opt(boxes.page_tree);
                return Ok(());
            }
        }
        boxes.page_tree = page;
        Ok(())
    }

    /// `page_by_number`.
    fn page_by_number(
        &mut self,
        page_tree: Option<Obj>,
        page_no: i32,
        boxes: &mut PdfBoxes,
    ) -> Result<()> {
        let mut page_tree = page_tree;
        let fail = |s: &mut Self, pt: Option<Obj>| s.o.release_opt(pt);
        let Some(pt0) = page_tree else { return Ok(()) };
        let v = self.o.lookup_dict(pt0, b"Count");
        let tmp = self.o.deref_obj(v)?;
        if !self.o.is_number(tmp) {
            self.o.release_opt(tmp);
            return Ok(fail(self, page_tree));
        }
        let count = self.o.number_value(tmp.expect("number")) as i32;
        self.o.release_opt(tmp);
        if page_no <= 0 || page_no > count {
            return Ok(fail(self, page_tree));
        }
        let mut depth = crate::obj::PDF_OBJ_MAX_DEPTH;
        let mut page_idx = page_no - 1;
        let mut kids_length = 1usize;
        let mut i = 0usize;
        loop {
            depth -= 1;
            if depth == 0 || i == kids_length {
                break;
            }
            boxes.page_tree = page_tree;
            self.get_page_properties(boxes)?;
            let v = self.o.lookup_dict(page_tree.expect("dict"), b"Kids");
            let kids = self.o.deref_obj(v)?;
            let Some(kids) = kids else { break };
            if !self.o.is_array(Some(kids)) {
                self.o.release(kids);
                return Ok(fail(self, page_tree));
            }
            kids_length = self.o.array_length(kids);
            i = 0;
            while i < kids_length {
                self.o.release_opt(page_tree);
                let e = self.o.get_array(kids, i as i32);
                page_tree = self.o.deref_obj(e)?;
                if !self.o.is_dict(page_tree) {
                    return Ok(fail(self, page_tree));
                }
                let v = self.o.lookup_dict(page_tree.expect("dict"), b"Count");
                let tmp = self.o.deref_obj(v)?;
                let count = if self.o.is_number(tmp) {
                    let c = self.o.number_value(tmp.expect("number")) as i32;
                    self.o.release_opt(tmp);
                    c
                } else if tmp.is_none() {
                    1
                } else {
                    self.o.release_opt(tmp);
                    return Ok(fail(self, page_tree));
                };
                if page_idx < count {
                    break;
                }
                page_idx -= count;
                i += 1;
            }
            self.o.release(kids);
        }
        if depth == 0 || kids_length == i {
            return Ok(fail(self, page_tree));
        }
        boxes.page_tree = page_tree;
        Ok(())
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
    ) -> Result<(Option<Obj>, PdfRect, PdfTmatrix, Option<Obj>)> {
        let mut boxes = PdfBoxes::default();
        let mut bbox = PdfRect::default();
        let mut matrix = PdfTmatrix::default();
        let mut resources = None;
        let catalog = self.o.pdf_file_catalog(pf).expect("catalog");
        let mut page_tree;
        let ok = 'get: {
            if let Some(name) = page_name {
                self.page_by_name(catalog, name, &mut boxes)?;
            } else if page_no > 0 {
                let v = self.o.lookup_dict(catalog, b"Pages");
                let pt = self.o.deref_obj(v)?;
                self.page_by_number(pt, page_no, &mut boxes)?;
            } else {
                page_tree = None;
                break 'get false;
            }
            page_tree = boxes.page_tree;
            if !self.o.is_dict(page_tree) || !self.o.is_dict(boxes.resources) {
                break 'get false;
            }
            if want_resources {
                resources = self.o.link_opt(boxes.resources);
            }
            let (e, b) = self.set_bounding_box(
                opt_bbox,
                boxes.media_box,
                boxes.crop_box,
                boxes.art_box,
                boxes.trim_box,
                boxes.bleed_box,
            )?;
            bbox = b;
            if e != 0 {
                break 'get false;
            }
            let (e, m) = self.set_transform_matrix(&mut bbox, boxes.rotate);
            matrix = m;
            e == 0
        };
        if !ok {
            self.o.release_opt(page_tree);
            page_tree = None;
        }
        for b in [
            boxes.crop_box,
            boxes.bleed_box,
            boxes.trim_box,
            boxes.art_box,
            boxes.media_box,
            boxes.rotate,
            boxes.resources,
        ] {
            self.o.release_opt(b);
        }
        Ok((page_tree, bbox, matrix, resources))
    }
}

/// C's `strcmp` (the strings end at their first NUL): its sign.
fn strcmp_c(a: &[u8], b: &[u8]) -> i32 {
    let a = a.iter().position(|&c| c == 0).map_or(a, |n| &a[..n]);
    let b = b.iter().position(|&c| c == 0).map_or(b, |n| &b[..n]);
    match a.cmp(b) {
        core::cmp::Ordering::Less => -1,
        core::cmp::Ordering::Equal => 0,
        core::cmp::Ordering::Greater => 1,
    }
}
