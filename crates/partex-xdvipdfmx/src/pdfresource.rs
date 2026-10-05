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
        self.ident = None;
        self.category = -1;
        self.flags = 0;
        self.object = None;
        self.reference = None;
    }
}

/// `get_category`: the category id, or -1.
#[must_use]
pub fn get_category(category: &[u8]) -> i32 {
    PDF_RESOURCE_CATEGORIES
        .iter()
        .find(|(n, _)| *n == category)
        .map_or(-1, |&(_, id)| id)
}

impl Dpx {
    /// `pdf_flush_resource`: releases its reference and object.
    fn pdf_flush_resource(&mut self, cat_id: usize, res_id: usize) {
        let res = &mut self.resource.resources[cat_id].resources[res_id];
        let (r, o) = (res.reference.take(), res.object.take());
        self.o.release_opt(r);
        self.o.release_opt(o);
    }
    /// `pdf_clean_resource`.
    fn pdf_clean_resource(&mut self, cat_id: usize, res_id: usize) {
        let res = &mut self.resource.resources[cat_id].resources[res_id];
        let (r, o) = (res.reference.take(), res.object.take());
        res.ident = None;
        res.category = -1;
        res.flags = 0;
        self.o.release_opt(r);
        self.o.release_opt(o);
    }
    /// `pdf_init_resources`.
    pub fn pdf_init_resources(&mut self) {
        for rc in &mut self.resource.resources {
            rc.resources.clear();
        }
    }
    /// `pdf_close_resources`.
    pub fn pdf_close_resources(&mut self) {
        for i in 0..PDF_NUM_RESOURCE_CATEGORIES {
            for j in 0..self.resource.resources[i].resources.len() {
                self.pdf_flush_resource(i, j);
                self.pdf_clean_resource(i, j);
            }
            self.resource.resources[i].resources.clear();
        }
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
        let cat_id = get_category(category);
        if cat_id < 0 {
            crate::error!("Unknown resource category: {:?}", category);
        }
        let c = cat_id as usize;
        let count = self.resource.resources[c].resources.len();
        let mut res_id = count;
        if let Some(name) = resname {
            for i in 0..count {
                // (C's strcmp with an unnamed resource's NULL ident would
                // crash; such a resource never matches here.)
                if self.resource.resources[c].resources[i].ident.as_deref() == Some(name) {
                    self.pdf_flush_resource(c, i);
                    let reference = if flags & PDF_RES_FLUSH_IMMEDIATE != 0 {
                        let r = self.o.ref_obj(object);
                        self.o.release(object);
                        Some(r)
                    } else {
                        None
                    };
                    let res = &mut self.resource.resources[c].resources[i];
                    res.flags = flags;
                    if reference.is_some() {
                        res.reference = reference;
                    } else {
                        res.object = Some(object);
                    }
                    return (cat_id << 16) | i as i32;
                }
            }
            res_id = count;
        }
        let mut res = PdfRes::default();
        res.pdf_init_resource();
        if let Some(name) = resname
            && !name.is_empty()
        {
            res.ident = Some(name.to_vec());
        }
        res.category = cat_id;
        res.flags = flags;
        if flags & PDF_RES_FLUSH_IMMEDIATE != 0 {
            res.reference = Some(self.o.ref_obj(object));
            self.o.release(object);
        } else {
            res.object = Some(object);
        }
        self.resource.resources[c].resources.push(res);
        (cat_id << 16) | res_id as i32
    }
    /// `pdf_findresource`: the resource id, or -1.
    pub fn pdf_findresource(&mut self, category: &[u8], resname: &[u8]) -> i32 {
        let cat_id = get_category(category);
        if cat_id < 0 {
            crate::error!("Unknown resource category: {:?}", category);
        }
        self.resource.resources[cat_id as usize]
            .resources
            .iter()
            .position(|r| r.ident.as_deref() == Some(resname))
            .map_or(-1, |i| (cat_id << 16) | i as i32)
    }
    /// `pdf_get_resource_reference`: a new link to the reference.
    pub fn pdf_get_resource_reference(&mut self, rc_id: i32) -> Option<Obj> {
        let c = ((rc_id >> 16) & 0xffff) as usize;
        let r = (rc_id & 0xffff) as usize;
        if c >= PDF_NUM_RESOURCE_CATEGORIES {
            crate::error!("Invalid category ID: {}", c);
        }
        if r >= self.resource.resources[c].resources.len() {
            crate::error!("Invalid resource ID: {}", r);
        }
        let res = &self.resource.resources[c].resources[r];
        let reference = match res.reference {
            Some(x) => x,
            None => {
                let Some(ob) = res.object else {
                    crate::error!("Undefined object...")
                };
                let x = self.o.ref_obj(ob);
                self.resource.resources[c].resources[r].reference = Some(x);
                x
            }
        };
        Some(self.o.link(reference))
    }
}
