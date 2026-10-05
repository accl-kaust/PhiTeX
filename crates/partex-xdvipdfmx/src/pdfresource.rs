//! pdfresource.c, pdfresource.h: named resources (fonts, XObjects, …) by
//! category; a resource id is `cat_id << 16 | res_id`.

use crate::prelude::*;

/// `PDF_RES_FLUSH_IMMEDIATE`.
pub const PDF_RES_FLUSH_IMMEDIATE: i32 = 1;

pub const PDF_RESOURCE_FONT: i32 = 0;
pub const PDF_RESOURCE_CIDFONT: i32 = 1;
pub const PDF_RESOURCE_ENCODING: i32 = 2;
pub const PDF_RESOURCE_CMAP: i32 = 3;
pub const PDF_RESOURCE_XOBJECT: i32 = 4;
pub const PDF_RESOURCE_COLORSPACE: i32 = 5;
pub const PDF_RESOURCE_SHADING: i32 = 6;
pub const PDF_RESOURCE_PATTERN: i32 = 7;
pub const PDF_RESOURCE_GSTATE: i32 = 8;

/// `pdf_resource_categories[]`: (name, cat_id).
pub const PDF_RESOURCE_CATEGORIES: [(&[u8], i32); 9] = [
    (b"Font", PDF_RESOURCE_FONT),
    (b"CIDFont", PDF_RESOURCE_CIDFONT),
    (b"Encoding", PDF_RESOURCE_ENCODING),
    (b"CMap", PDF_RESOURCE_CMAP),
    (b"XObject", PDF_RESOURCE_XOBJECT),
    (b"ColorSpace", PDF_RESOURCE_COLORSPACE),
    (b"Shading", PDF_RESOURCE_SHADING),
    (b"Pattern", PDF_RESOURCE_PATTERN),
    (b"ExtGState", PDF_RESOURCE_GSTATE),
];

/// `PDF_NUM_RESOURCE_CATEGORIES`.
pub const PDF_NUM_RESOURCE_CATEGORIES: usize = 9;

/// `pdf_res` (C's `cdata` is never set: not kept).
#[derive(Clone, Debug, Default)]
pub struct PdfRes {
    pub ident: Option<Vec<u8>>,
    pub flags: i32,
    pub category: i32,
    pub object: Option<Obj>,
    pub reference: Option<Obj>,
}

/// `struct res_cache` (count/capacity: the Vec).
#[derive(Clone, Debug, Default)]
pub struct ResCache {
    pub resources: Vec<PdfRes>,
}

/// pdfresource.c's statics.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// `resources[PDF_NUM_RESOURCE_CATEGORIES]`.
    pub resources: [ResCache; PDF_NUM_RESOURCE_CATEGORIES],
}

impl PdfRes {
    /// `pdf_init_resource`.
    pub fn pdf_init_resource(&mut self) {
        todo!()
    }
}

/// `get_category`: the category id, or -1.
#[must_use]
pub fn get_category(category: &[u8]) -> i32 {
    todo!()
}

impl Dpx {
    /// `pdf_flush_resource`: releases its reference and object.
    fn pdf_flush_resource(&mut self, cat_id: usize, res_id: usize) {
        todo!()
    }
    /// `pdf_clean_resource`.
    fn pdf_clean_resource(&mut self, cat_id: usize, res_id: usize) {
        todo!()
    }
    /// `pdf_init_resources`.
    pub fn pdf_init_resources(&mut self) {
        todo!()
    }
    /// `pdf_close_resources`.
    pub fn pdf_close_resources(&mut self) {
        todo!()
    }
    /// `pdf_defineresource`: the resource id (`object` is owned by the
    /// cache).
    pub fn pdf_defineresource(
        &mut self,
        category: &[u8],
        resname: Option<&[u8]>,
        object: Obj,
        flags: i32,
    ) -> i32 {
        todo!()
    }
    /// `pdf_findresource`: the resource id, or -1.
    pub fn pdf_findresource(&mut self, category: &[u8], resname: &[u8]) -> i32 {
        todo!()
    }
    /// `pdf_get_resource_reference`: a new link to the reference.
    pub fn pdf_get_resource_reference(&mut self, rc_id: i32) -> Option<Obj> {
        todo!()
    }
}
