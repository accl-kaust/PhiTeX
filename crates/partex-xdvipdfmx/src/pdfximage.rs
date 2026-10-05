//! pdfximage.c, pdfximage.h: the XObjects (images and forms) cache.
//!
//! A `pdf_ximage *` is an `xobj_id: i32`, the index into
//! `self.ximage.ximages`. The loaders (pngimage, jpegimage, epdf) take the
//! id of the entry `load_image` made and fill it with
//! `pdf_ximage_set_image` / `pdf_ximage_set_form`. jp2image, bmpimage,
//! mpost (MetaPost/EPS) and the `ps_include_page` distiller are not ported.

use crate::pdfdev::{PdfRect, PdfTmatrix, TransformInfo};
use crate::prelude::*;

/// `PDF_XOBJECT_TYPE_FORM`.
pub const PDF_XOBJECT_TYPE_FORM: i32 = 0;
/// `PDF_XOBJECT_TYPE_IMAGE`.
pub const PDF_XOBJECT_TYPE_IMAGE: i32 = 1;

/// `IMAGE_TYPE_UNKNOWN` (pdfximage.c's `source_image_type` results).
pub const IMAGE_TYPE_UNKNOWN: i32 = -1;
pub const IMAGE_TYPE_PDF: i32 = 0;
pub const IMAGE_TYPE_JPEG: i32 = 1;
pub const IMAGE_TYPE_PNG: i32 = 2;
pub const IMAGE_TYPE_MPS: i32 = 4;
pub const IMAGE_TYPE_EPS: i32 = 5;
pub const IMAGE_TYPE_BMP: i32 = 6;
pub const IMAGE_TYPE_JP2: i32 = 7;

/// `EBB_DPI`.
pub const EBB_DPI: i32 = 72;

/// `ximage_info`. C's `pdf_ximage_init_image_info` sets the densities to
/// 1.0 (`Default` gives zeros: use [`pdf_ximage_init_image_info`]).
#[derive(Clone, Debug, Default)]
pub struct XimageInfo {
    pub flags: i32,
    pub width: i32,
    pub height: i32,
    pub bits_per_component: i32,
    pub num_components: i32,
    /// Not used yet.
    pub min_dpi: i32,
    /// Scale factors for bp.
    pub xdensity: f64,
    pub ydensity: f64,
}

/// `xform_info`. Use [`pdf_ximage_init_form_info`] for C's initial values.
#[derive(Clone, Debug, Default)]
pub struct XformInfo {
    pub flags: i32,
    pub bbox: PdfRect,
    pub matrix: PdfTmatrix,
}

/// What `pdf_ximage_defineresource`'s `void *cdata` points to (by subtype).
#[derive(Clone, Debug)]
pub enum XobjInfo {
    Image(XimageInfo),
    Form(XformInfo),
}

/// `load_options`.
#[derive(Clone, Debug, Default)]
pub struct LoadOptions {
    pub page_no: i32,
    /// `enum pdf_page_boundary` (pdfdoc.h): 0 auto, 1 mediabox, 2 cropbox,
    /// 3 artbox, 4 trimbox, 5 bleedbox.
    pub bbox_type: i32,
    pub dict: Option<Obj>,
    pub page_name: Option<Vec<u8>>,
}

/// `struct attr_` (pdfximage.c).
#[derive(Clone, Debug, Default)]
pub struct XimageAttr {
    pub width: i32,
    pub height: i32,
    pub xdensity: f64,
    pub ydensity: f64,
    pub bbox: PdfRect,
    pub page_no: i32,
    pub page_count: i32,
    /// "Ugh": an `enum pdf_page_boundary` value.
    pub bbox_type: i32,
    pub dict: Option<Obj>,
    /// `char tempfile`: the file is a temporary (distiller output).
    pub tempfile: bool,
    pub page_name: Option<Vec<u8>>,
}

/// `struct pdf_ximage_`. `Default` is not C's initial state: use
/// [`PdfXimage::pdf_init_ximage_struct`].
#[derive(Clone, Debug, Default)]
pub struct PdfXimage {
    pub ident: Option<Vec<u8>>,
    /// `char res_name[16]`: the resource name (`Im1`, `Fm2`…), no NUL.
    pub res_name: Vec<u8>,
    pub subtype: i32,
    pub attr: XimageAttr,
    pub filename: Option<Vec<u8>>,
    pub fullname: Option<Vec<u8>>,
    pub reference: Option<Obj>,
    pub resource: Option<Obj>,
    pub reserved: i32,
}

/// pdfximage.c's statics: `_opts` and `_ic`.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// `_opts.cmdtmpl`: the distiller command template.
    pub cmdtmpl: Option<Vec<u8>>,
    /// `_ic.ximages` (`count` is the length), indexed by `xobj_id`.
    pub ximages: Vec<PdfXimage>,
}

impl PdfXimage {
    /// `pdf_init_ximage_struct`: C's initial values (subtype -1, densities
    /// 1.0, page 1 of 1…).
    #[must_use]
    pub fn pdf_init_ximage_struct() -> Self {
        todo!()
    }
    /// `pdf_set_ximage_tempfile`.
    pub fn pdf_set_ximage_tempfile(&mut self, fullname: &[u8]) {
        todo!()
    }
}

/// `pdf_ximage_init_image_info`.
#[must_use]
pub fn pdf_ximage_init_image_info() -> XimageInfo {
    todo!()
}

/// `pdf_ximage_init_form_info`.
#[must_use]
pub fn pdf_ximage_init_form_info() -> XformInfo {
    todo!()
}

/// `source_image_type` (static): an `IMAGE_TYPE_*`; rewinds `fp`.
pub fn source_image_type(fp: &mut MemFile) -> i32 {
    todo!()
}

/// `check_for_ps` (static).
pub fn check_for_ps(fp: &mut MemFile) -> i32 {
    todo!()
}

/// `check_for_mp` (static).
pub fn check_for_mp(fp: &mut MemFile) -> i32 {
    todo!()
}

/// `scale_to_fit_I` (static): for images.
#[allow(non_snake_case)]
pub fn scale_to_fit_I(t: &mut PdfTmatrix, p: &TransformInfo, i: &PdfXimage) {
    todo!()
}

/// `scale_to_fit_F` (static): for forms.
#[allow(non_snake_case)]
pub fn scale_to_fit_F(t: &mut PdfTmatrix, p: &TransformInfo, i: &PdfXimage) {
    todo!()
}

impl Dpx {
    /// `pdf_init_images`.
    pub fn pdf_init_images(&mut self) {
        todo!()
    }
    /// `pdf_close_images`: flushes and releases every XObject.
    pub fn pdf_close_images(&mut self) {
        todo!()
    }
    /// `pdf_clean_ximage_struct` (static): releases the entry's objects
    /// and re-initialises it.
    pub fn pdf_clean_ximage_struct(&mut self, xobj_id: i32) {
        todo!()
    }
    /// `load_image` (static): the id, or -1. Dispatches on `format`
    /// (`IMAGE_TYPE_*`); BMP, JP2, MPS and EPS are not ported.
    pub fn load_image(
        &mut self,
        ident: Option<&[u8]>,
        filename: Option<&[u8]>,
        fullname: &[u8],
        format: i32,
        fp: &mut MemFile,
        options: LoadOptions,
    ) -> i32 {
        todo!()
    }
    /// `pdf_ximage_load_image`: the id, or -1.
    pub fn pdf_ximage_load_image(
        &mut self,
        ident: Option<&[u8]>,
        filename: &[u8],
        options: LoadOptions,
    ) -> i32 {
        todo!()
    }
    /// `pdf_ximage_findresource`: the id, or -1.
    pub fn pdf_ximage_findresource(&mut self, ident: &[u8]) -> i32 {
        todo!()
    }
    /// `pdf_ximage_set_image`: `resource` is the image stream.
    pub fn pdf_ximage_set_image(&mut self, xobj_id: i32, info: &XimageInfo, resource: Obj) {
        todo!()
    }
    /// `pdf_ximage_set_form`: `resource` is the form stream.
    pub fn pdf_ximage_set_form(&mut self, xobj_id: i32, info: &XformInfo, resource: Obj) {
        todo!()
    }
    /// `pdf_ximage_get_page`.
    pub fn pdf_ximage_get_page(&mut self, xobj_id: i32) -> i32 {
        todo!()
    }
    /// `pdf_ximage_get_reference`: a new link to the reference (made if
    /// none yet).
    pub fn pdf_ximage_get_reference(&mut self, xobj_id: i32) -> Obj {
        todo!()
    }
    /// `pdf_ximage_defineresource`: the id. `cdata` matches `subtype`.
    pub fn pdf_ximage_defineresource(
        &mut self,
        ident: Option<&[u8]>,
        subtype: i32,
        cdata: &XobjInfo,
        resource: Obj,
    ) -> i32 {
        todo!()
    }
    /// `pdf_ximage_reserve`: the id of a reserved (forward-referenced)
    /// XObject.
    pub fn pdf_ximage_reserve(&mut self, ident: &[u8]) -> i32 {
        todo!()
    }
    /// `pdf_ximage_get_resname`: a copy of the resource name.
    pub fn pdf_ximage_get_resname(&mut self, xobj_id: i32) -> Vec<u8> {
        todo!()
    }
    /// `pdf_ximage_get_subtype`.
    pub fn pdf_ximage_get_subtype(&mut self, xobj_id: i32) -> i32 {
        todo!()
    }
    /// `pdf_ximage_set_attr`.
    pub fn pdf_ximage_set_attr(
        &mut self,
        xobj_id: i32,
        width: i32,
        height: i32,
        xdensity: f64,
        ydensity: f64,
        llx: f64,
        lly: f64,
        urx: f64,
        ury: f64,
    ) {
        todo!()
    }
    /// `pdf_ximage_scale_image`: status, the matrix `M` and the clipping
    /// rectangle `r`, from the special's `p`.
    pub fn pdf_ximage_scale_image(
        &mut self,
        xobj_id: i32,
        p: &TransformInfo,
    ) -> (i32, PdfTmatrix, PdfRect) {
        todo!()
    }
    /// `set_distiller_template`.
    pub fn set_distiller_template(&mut self, s: Option<&[u8]>) {
        todo!()
    }
    /// `get_distiller_template`: a copy.
    pub fn get_distiller_template(&mut self) -> Option<Vec<u8>> {
        todo!()
    }
    /// `ps_include_page` (static): runs the distiller on a PS/EPS file —
    /// not ported (no external commands).
    pub fn ps_include_page(&mut self, xobj_id: i32, filename: &[u8], options: LoadOptions) -> i32 {
        todo!()
    }
    /// `pdf_error_cleanup_cache`: deletes the temporary files (none here).
    pub fn pdf_error_cleanup_cache(&mut self) {
        todo!()
    }
}
