//! pdfximage.c, pdfximage.h: the XObjects (images and forms) cache.
//!
//! A `pdf_ximage *` is an `xobj_id: i32`, the index into
//! `self.ximage.ximages`. The loaders (pngimage, jpegimage, bmpimage, epdf)
//! take the id of the entry `load_image` made and fill it with
//! `pdf_ximage_set_image` / `pdf_ximage_set_form`. jp2image, mpost
//! (MetaPost/EPS) and the `ps_include_page` distiller are not ported.

use crate::ctx::CompatMode;
use crate::dpxutil::{max4, min4};
use crate::io::Format;
use crate::pdfdev::{
    INFO_HAS_HEIGHT, INFO_HAS_USER_BBOX, INFO_HAS_WIDTH, PdfCoord, PdfRect, PdfTmatrix,
    TransformInfo,
};
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
        PdfXimage {
            ident: None,
            res_name: Vec::new(),
            subtype: -1,
            attr: XimageAttr {
                width: 0,
                height: 0,
                xdensity: 1.0,
                ydensity: 1.0,
                bbox: PdfRect::default(),
                page_no: 1,
                page_count: 1,
                bbox_type: 0,
                dict: None,
                tempfile: false,
                page_name: None,
            },
            filename: None,
            fullname: None,
            reference: None,
            resource: None,
            reserved: 0,
        }
    }
    /// `pdf_set_ximage_tempfile`.
    pub fn pdf_set_ximage_tempfile(&mut self, fullname: &[u8]) {
        self.fullname = Some(fullname.to_vec());
        self.attr.tempfile = true;
    }
}

/// `pdf_ximage_init_image_info`.
#[must_use]
pub fn pdf_ximage_init_image_info() -> XimageInfo {
    XimageInfo {
        flags: 0,
        width: 0,
        height: 0,
        bits_per_component: 0,
        num_components: 0,
        min_dpi: 0,
        xdensity: 1.0,
        ydensity: 1.0,
    }
}

/// `pdf_ximage_init_form_info`.
#[must_use]
pub fn pdf_ximage_init_form_info() -> XformInfo {
    XformInfo {
        flags: 0,
        bbox: PdfRect {
            llx: 0.0,
            lly: 0.0,
            urx: 0.0,
            ury: 0.0,
        },
        matrix: PdfTmatrix {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: 0.0,
            f: 0.0,
        },
    }
}

/// `check_for_jp2` (jp2image.c, with its `check_jp___box`,
/// `read_box_hdr` and `check_ftyp_data`; JPEG 2000 images are not
/// ported, but a JP2 file must be told from the others). Reading past
/// the end stops the run, as numbers.c's readers do.
fn check_for_jp2(fp: &mut MemFile) -> bool {
    const JP2_BOX_JP__: u32 = 0x6a50_2020;
    const JP2_BOX_FTYP: u32 = 0x6674_7970;
    const FTYP_BR_JP2_: u32 = 0x6a70_3220;
    const FTYP_BR_JPX_: u32 = 0x6a70_7820;
    const FTYP_CL_JPXB: u32 = 0x6a70_7862;

    fp.rewind();

    /* JPEG 2000 Singature box */
    if fp.get_unsigned_quad() != 0x0c
        || fp.get_unsigned_quad() != JP2_BOX_JP__
        || fp.get_unsigned_quad() != 0x0D0A_870A
    {
        return false;
    }

    /* File Type box shall immediately follow */
    let mut lbox = fp.get_unsigned_quad();
    let tbox = fp.get_unsigned_quad();
    let mut len: u32 = 8;
    if lbox == 1 {
        if fp.get_unsigned_quad() != 0 {
            error!("JPEG2000: LBox value in JP2 file >32 bits.\nI can't handle this!");
        }
        lbox = fp.get_unsigned_quad();
        len += 8;
    }
    if tbox != JP2_BOX_FTYP {
        return false;
    }
    let mut size = lbox.wrapping_sub(len);
    let br = fp.get_unsigned_quad();
    size = size.wrapping_sub(4);
    /* MinV = */
    fp.get_unsigned_quad();
    size = size.wrapping_sub(4);
    match br {
        FTYP_BR_JP2_ => {
            fp.seek_relative(size as i32 as isize);
            true
        }
        FTYP_BR_JPX_ => {
            let mut supported = false;
            while size > 0 {
                if fp.get_unsigned_quad() == FTYP_CL_JPXB {
                    supported = true;
                }
                size = size.wrapping_sub(4);
            }
            supported
        }
        _ => {
            fp.seek_relative(size as i32 as isize);
            false
        }
    }
}

/// `check_for_pdf` (pdfobj.c): any `%PDF-M.N` file, whatever its version.
fn check_for_pdf(fp: &mut MemFile) -> bool {
    fp.rewind();
    crate::pdfread::check_for_pdf_version(&fp.data) >= 0
}

/// `source_image_type` (static): an `IMAGE_TYPE_*`; rewinds `fp`.
pub fn source_image_type(fp: &mut MemFile) -> i32 {
    fp.rewind();
    /*
     * Make sure we check for PS *after* checking for MP since
     * MP is a special case of PS.
     */
    let format = if crate::jpegimage::check_for_jpeg(fp) != 0 {
        IMAGE_TYPE_JPEG
    } else if check_for_jp2(fp) {
        IMAGE_TYPE_JP2
    } else if crate::pngimage::check_for_png(fp) != 0 {
        IMAGE_TYPE_PNG
    } else if crate::bmpimage::check_for_bmp(fp) {
        IMAGE_TYPE_BMP
    } else if check_for_pdf(fp) {
        IMAGE_TYPE_PDF
    } else if check_for_mp(fp) != 0 {
        IMAGE_TYPE_MPS
    } else if check_for_ps(fp) != 0 {
        IMAGE_TYPE_EPS
    } else {
        IMAGE_TYPE_UNKNOWN
    };
    fp.rewind();

    format
}

/// `WORK_BUFFER_SIZE` (mfileio.h).
const WORK_BUFFER_SIZE: usize = 1024;

/// `check_for_ps` (static).
pub fn check_for_ps(fp: &mut MemFile) -> i32 {
    fp.rewind();
    let line = fp.mfgets(WORK_BUFFER_SIZE).unwrap_or_default();
    if line.starts_with(b"%!") {
        return 1;
    }
    0
}

/// `check_for_mp` (static).
pub fn check_for_mp(fp: &mut MemFile) -> i32 {
    let mut try_count = 10;

    fp.rewind();
    let line = fp.mfgets(WORK_BUFFER_SIZE).unwrap_or_default();
    if !line.starts_with(b"%!PS") {
        return 0;
    }

    while try_count > 0 {
        let line = fp.mfgets(WORK_BUFFER_SIZE).unwrap_or_default();
        if line.starts_with(b"%%Creator:") {
            let rest = &line[10..];
            if rest.len() >= 8 && rest.windows(8).any(|w| w == b"MetaPost") {
                break;
            }
        }
        try_count -= 1;
    }

    i32::from(try_count > 0)
}

/// `scale_to_fit_I` (static): for images.
#[allow(non_snake_case)]
pub fn scale_to_fit_I(t: &mut PdfTmatrix, p: &TransformInfo, i: &PdfXimage) {
    let (mut wd0, mut ht0, xscale, yscale, d_x, d_y);
    if p.flags & INFO_HAS_USER_BBOX != 0 {
        wd0 = p.bbox.urx - p.bbox.llx;
        ht0 = p.bbox.ury - p.bbox.lly;
        xscale = f64::from(i.attr.width) * i.attr.xdensity / wd0;
        yscale = f64::from(i.attr.height) * i.attr.ydensity / ht0;
        d_x = -p.bbox.llx / wd0;
        d_y = -p.bbox.lly / ht0;
    } else {
        wd0 = f64::from(i.attr.width) * i.attr.xdensity;
        ht0 = f64::from(i.attr.height) * i.attr.ydensity;
        xscale = 1.0;
        yscale = 1.0;
        d_x = 0.0;
        d_y = 0.0;
    }

    if wd0 == 0.0 {
        warn!("Image width=0.0!");
        wd0 = 1.0;
    }
    if ht0 == 0.0 {
        warn!("Image height=0.0!");
        ht0 = 1.0;
    }

    let (s_x, s_y, dp);
    if p.flags & INFO_HAS_WIDTH != 0 && p.flags & INFO_HAS_HEIGHT != 0 {
        s_x = p.width * xscale;
        s_y = (p.height + p.depth) * yscale;
        dp = p.depth * yscale;
    } else if p.flags & INFO_HAS_WIDTH != 0 {
        s_x = p.width * xscale;
        s_y = s_x * (f64::from(i.attr.height) / f64::from(i.attr.width));
        dp = 0.0;
    } else if p.flags & INFO_HAS_HEIGHT != 0 {
        s_y = (p.height + p.depth) * yscale;
        s_x = s_y * (f64::from(i.attr.width) / f64::from(i.attr.height));
        dp = p.depth * yscale;
    } else {
        s_x = wd0;
        s_y = ht0;
        dp = 0.0;
    }
    t.a = s_x;
    t.c = 0.0;
    t.b = 0.0;
    t.d = s_y;
    t.e = d_x * s_x / xscale;
    t.f = d_y * s_y / yscale - dp;
}

/// `scale_to_fit_F` (static): for forms.
#[allow(non_snake_case)]
pub fn scale_to_fit_F(t: &mut PdfTmatrix, p: &TransformInfo, i: &PdfXimage) {
    let (mut wd0, mut ht0, d_x, d_y);
    if p.flags & INFO_HAS_USER_BBOX != 0 {
        wd0 = p.bbox.urx - p.bbox.llx;
        ht0 = p.bbox.ury - p.bbox.lly;
        d_x = -p.bbox.llx;
        d_y = -p.bbox.lly;
    } else {
        wd0 = i.attr.bbox.urx - i.attr.bbox.llx;
        ht0 = i.attr.bbox.ury - i.attr.bbox.lly;
        d_x = 0.0;
        d_y = 0.0;
    }

    if wd0 == 0.0 {
        warn!("Image width=0.0!");
        wd0 = 1.0;
    }
    if ht0 == 0.0 {
        warn!("Image height=0.0!");
        ht0 = 1.0;
    }

    let (s_x, s_y, dp);
    if p.flags & INFO_HAS_WIDTH != 0 && p.flags & INFO_HAS_HEIGHT != 0 {
        s_x = p.width / wd0;
        s_y = (p.height + p.depth) / ht0;
        dp = p.depth;
    } else if p.flags & INFO_HAS_WIDTH != 0 {
        s_x = p.width / wd0;
        s_y = s_x;
        dp = 0.0;
    } else if p.flags & INFO_HAS_HEIGHT != 0 {
        s_y = (p.height + p.depth) / ht0;
        s_x = s_y;
        dp = p.depth;
    } else {
        s_x = 1.0;
        s_y = 1.0;
        dp = 0.0;
    }

    t.a = s_x;
    t.c = 0.0;
    t.b = 0.0;
    t.d = s_y;
    t.e = s_x * d_x;
    t.f = s_y * d_y - dp;
}

/// `sprintf(I->res_name, "Im%d"/"Fm%d", id)`.
fn res_name(prefix: &[u8], id: i32) -> Vec<u8> {
    let mut b = crate::fmt::Buf::new();
    b.extend(prefix);
    b.int(id);
    b.0
}

impl Dpx {
    /// `CHECK_ID`.
    fn ximage_check_id(&self, id: i32) {
        if id < 0 || id as usize >= self.ximage.ximages.len() {
            error!("Invalid XObject ID: {id}");
        }
    }
    /// `pdf_init_images`.
    pub fn pdf_init_images(&mut self) {
        self.ximage.ximages = Vec::new();
    }
    /// `pdf_close_images`: flushes and releases every XObject. (No image
    /// is ever a temporary file here: the distiller is not ported.)
    pub fn pdf_close_images(&mut self) {
        if !self.ximage.ximages.is_empty() {
            for i in 0..self.ximage.ximages.len() {
                if self.ximage.ximages[i].attr.tempfile {
                    /* dpx_delete_temp_file(): nothing to delete here */
                    self.ximage.ximages[i].fullname = None;
                }
                self.pdf_clean_ximage_struct(i as i32);
            }
            self.ximage.ximages = Vec::new();
        }

        self.ximage.cmdtmpl = None;
    }
    /// `pdf_clean_ximage_struct` (static): releases the entry's objects
    /// and re-initialises it.
    pub fn pdf_clean_ximage_struct(&mut self, xobj_id: i32) {
        let old = core::mem::replace(
            &mut self.ximage.ximages[xobj_id as usize],
            PdfXimage::pdf_init_ximage_struct(),
        );
        self.o.release_opt(old.reference);
        self.o.release_opt(old.resource);
        self.o.release_opt(old.attr.dict); /* unsafe? */
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
        let mut id: i32 = -1;
        let mut reserved = false;

        if let Some(ident) = ident {
            for (i, x) in self.ximage.ximages.iter().enumerate() {
                if x.ident.as_deref() == Some(ident) && x.reserved != 0 {
                    id = i as i32;
                    reserved = true;
                    break;
                }
            }
        }
        if !reserved {
            id = self.ximage.ximages.len() as i32;
            let mut x = PdfXimage::pdf_init_ximage_struct();
            if let Some(ident) = ident {
                x.ident = Some(ident.to_vec());
            }
            self.ximage.ximages.push(x);
        }
        {
            let x = &mut self.ximage.ximages[id as usize];
            if let Some(filename) = filename {
                x.filename = Some(filename.to_vec());
            }
            x.fullname = Some(fullname.to_vec());

            x.attr.page_no = options.page_no;
            x.attr.page_name.clone_from(&options.page_name);
            x.attr.bbox_type = options.bbox_type;
            x.attr.dict = options.dict; /* unsafe? */
        }

        let ok = match format {
            IMAGE_TYPE_JPEG => {
                if self.jpeg_include_image(id, fp) < 0 {
                    false
                } else {
                    self.ximage.ximages[id as usize].subtype = PDF_XOBJECT_TYPE_IMAGE;
                    true
                }
            }
            IMAGE_TYPE_JP2 => todo!("JPEG 2000 images (jp2image.c) are not ported"),
            IMAGE_TYPE_PNG => {
                if self.png_include_image(id, fp) < 0 {
                    false
                } else {
                    self.ximage.ximages[id as usize].subtype = PDF_XOBJECT_TYPE_IMAGE;
                    true
                }
            }
            IMAGE_TYPE_BMP => {
                if self.bmp_include_image(id, fp) < 0 {
                    false
                } else {
                    self.ximage.ximages[id as usize].subtype = PDF_XOBJECT_TYPE_IMAGE;
                    true
                }
            }
            IMAGE_TYPE_PDF => {
                let mut result = self.pdf_include_page(id, fp, fullname, options.clone());
                if result > 0 {
                    /* PDF version too recent */
                    result = self.ps_include_page(id, fullname, options.clone());
                }
                if result < 0 {
                    false
                } else {
                    self.ximage.ximages[id as usize].subtype = PDF_XOBJECT_TYPE_FORM;
                    true
                }
            }
            /*
             * case  IMAGE_TYPE_EPS:
             */
            _ => {
                if self.ps_include_page(id, fullname, options.clone()) < 0 {
                    false
                } else {
                    self.ximage.ximages[id as usize].subtype = PDF_XOBJECT_TYPE_FORM;
                    true
                }
            }
        };
        if !ok {
            self.pdf_clean_ximage_struct(id);
            return -1;
        }

        let x = &mut self.ximage.ximages[id as usize];
        match x.subtype {
            PDF_XOBJECT_TYPE_IMAGE => x.res_name = res_name(b"Im", id),
            PDF_XOBJECT_TYPE_FORM => x.res_name = res_name(b"Fm", id),
            t => {
                error!("Unknown XObject subtype: {t}");
            }
        }

        id
    }
    /// `pdf_ximage_load_image`: the id, or -1.
    pub fn pdf_ximage_load_image(
        &mut self,
        ident: Option<&[u8]>,
        filename: &[u8],
        options: LoadOptions,
    ) -> i32 {
        let mut id: i32 = -1;
        let mut f: Option<Vec<u8>> = None;

        for i in 0..self.ximage.ximages.len() {
            let x = &self.ximage.ximages[i];
            if x.filename.as_deref() != Some(filename) {
                continue;
            }
            id = i as i32;
            f.clone_from(&x.fullname);

            if x.attr.page_no != options.page_no {
                continue;
            }
            if x.attr.page_name != options.page_name {
                continue;
            }
            let d = x.attr.dict;
            if self.o.compare_object(d, options.dict) != 0 {
                /* ????? */
                continue;
            }
            if self.ximage.ximages[i].attr.bbox_type != options.bbox_type {
                continue;
            }

            return id;
        }
        let fullname = if let Some(f) = f {
            /* we already have converted this file; f is the temporary file name */
            f
        } else {
            /* try loading image */
            match self.files.find(filename, Format::Pict, b"dvipdfmx") {
                Some(p) => p,
                None => {
                    if self.conf.compat_mode == CompatMode::Compat {
                        warn!("Image inclusion failed. Could not find file");
                    } else {
                        error!(
                            "Image inclusion failed. Could not find file: {}",
                            String::from_utf8_lossy(filename)
                        );
                    }
                    return -1;
                }
            }
        };

        let Some(data) = self.files.read(&fullname) else {
            if self.conf.compat_mode == CompatMode::Compat {
                warn!("Error opening image file");
            } else {
                error!(
                    "Error opening image file \"{}\".",
                    String::from_utf8_lossy(filename)
                );
            }
            return -1;
        };
        let mut fp = MemFile::new(data, &fullname);

        let format = source_image_type(&mut fp);
        let page_no = options.page_no;
        if format == IMAGE_TYPE_MPS {
            todo!("MetaPost images (mpost.c's mps_include_page) are not ported");
        }
        id = self.load_image(ident, Some(filename), &fullname, format, &mut fp, options);

        if id < 0 {
            if self.conf.compat_mode == CompatMode::Compat {
                warn!("Image inclusion failed (page={page_no}).");
            } else if format == IMAGE_TYPE_PDF || format == IMAGE_TYPE_EPS {
                error!(
                    "Image inclusion failed for \"{}\" (page={page_no}).",
                    String::from_utf8_lossy(filename)
                );
            } else {
                error!(
                    "Image inclusion failed for \"{}\"",
                    String::from_utf8_lossy(filename)
                );
            }
        }

        id
    }
    /// `pdf_ximage_findresource`: the id, or -1.
    pub fn pdf_ximage_findresource(&mut self, ident: &[u8]) -> i32 {
        for (id, x) in self.ximage.ximages.iter().enumerate() {
            if x.ident.as_deref() == Some(ident) {
                return id as i32;
            }
        }

        -1
    }
    /// The XObject's reference made through `global_names` (C's code
    /// shared by `pdf_ximage_set_image` and `_set_form`).
    fn ximage_set_reference(&mut self, xobj_id: i32, resource: Obj) {
        let i = xobj_id as usize;
        if let Some(ident) = self.ximage.ximages[i].ident.clone() {
            let mut names = self
                .doc
                .global_names
                .take()
                .expect("global_names not initialised");
            let r = self.o.link(resource);
            let error = self.o.pdf_names_add_object(&mut names, &ident, r);
            if let Some(old) = self.ximage.ximages[i].reference.take() {
                self.o.release(old);
            }
            if error != 0 {
                let r = self.o.ref_obj(resource);
                self.ximage.ximages[i].reference = Some(r);
            } else {
                /* Need to create object reference before closing it */
                let r = self.o.pdf_names_lookup_reference(&mut names, &ident);
                self.ximage.ximages[i].reference = r;
                self.o.pdf_names_close_object(&mut names, &ident);
            }
            self.doc.global_names = Some(names);
            self.ximage.ximages[i].reserved = 0;
        } else {
            let r = self.o.ref_obj(resource);
            self.ximage.ximages[i].reference = Some(r);
        }
        self.o.release(resource); /* Caller don't know we are using reference. */
        self.ximage.ximages[i].resource = None;
    }
    /// `pdf_ximage_set_image`: `resource` is the image stream.
    pub fn pdf_ximage_set_image(&mut self, xobj_id: i32, info: &XimageInfo, resource: Obj) {
        if !self.o.is_stream(Some(resource)) {
            error!("Image XObject must be of stream type.");
        }

        let x = &mut self.ximage.ximages[xobj_id as usize];
        x.subtype = PDF_XOBJECT_TYPE_IMAGE;

        x.attr.width = info.width; /* The width of the image, in samples */
        x.attr.height = info.height; /* The height of the image, in samples */
        x.attr.xdensity = info.xdensity;
        x.attr.ydensity = info.ydensity;
        let attr_dict = x.attr.dict;

        let dict = self.o.stream_dict(resource);
        self.o.put_name(dict, b"Type", b"XObject");
        self.o.put_name(dict, b"Subtype", b"Image");
        self.o.put_number(dict, b"Width", f64::from(info.width));
        self.o.put_number(dict, b"Height", f64::from(info.height));
        if info.bits_per_component > 0 {
            /* Ignored for JPXDecode filter. FIXME */
            self.o.put_number(
                dict,
                b"BitsPerComponent",
                f64::from(info.bits_per_component),
            );
        }
        if let Some(d) = attr_dict {
            self.o.merge_dict(dict, d);
        }

        self.ximage_set_reference(xobj_id, resource);
    }
    /// `pdf_ximage_set_form`: `resource` is the form stream.
    ///
    /// C sets `p1.y` again where it means `p2.y`: `p1.y` is the bbox's
    /// `lly` untransformed, and `p2.y` is never set (whatever the stack
    /// held: 0 here).
    pub fn pdf_ximage_set_form(&mut self, xobj_id: i32, info: &XformInfo, resource: Obj) {
        self.ximage.ximages[xobj_id as usize].subtype = PDF_XOBJECT_TYPE_FORM;

        /* Image's attribute "bbox" here is affected by /Rotate entry of included
         * PDF page.
         */
        let mut p1 = PdfCoord {
            x: info.bbox.llx,
            y: info.bbox.lly,
        };
        self.pdf_dev_transform(&mut p1, Some(&info.matrix));
        let mut p2 = PdfCoord {
            x: info.bbox.urx,
            y: 0.0,
        };
        p1.y = info.bbox.lly;
        self.pdf_dev_transform(&mut p2, Some(&info.matrix));
        let mut p3 = PdfCoord {
            x: info.bbox.urx,
            y: info.bbox.ury,
        };
        self.pdf_dev_transform(&mut p3, Some(&info.matrix));
        let mut p4 = PdfCoord {
            x: info.bbox.llx,
            y: info.bbox.ury,
        };
        self.pdf_dev_transform(&mut p4, Some(&info.matrix));

        let bbox = &mut self.ximage.ximages[xobj_id as usize].attr.bbox;
        bbox.llx = min4(p1.x, p2.x, p3.x, p4.x);
        bbox.lly = min4(p1.y, p2.y, p3.y, p4.y);
        bbox.urx = max4(p1.x, p2.x, p3.x, p4.x);
        bbox.ury = max4(p1.y, p2.y, p3.y, p4.y);

        self.ximage_set_reference(xobj_id, resource);
    }
    /// `pdf_ximage_get_page`.
    pub fn pdf_ximage_get_page(&mut self, xobj_id: i32) -> i32 {
        self.ximage.ximages[xobj_id as usize].attr.page_no
    }
    /// `pdf_ximage_get_reference`: a new link to the reference (made if
    /// none yet).
    pub fn pdf_ximage_get_reference(&mut self, xobj_id: i32) -> Obj {
        self.ximage_check_id(xobj_id);

        let i = xobj_id as usize;
        if self.ximage.ximages[i].reference.is_none()
            && let Some(res) = self.ximage.ximages[i].resource
        {
            let r = self.o.ref_obj(res);
            self.ximage.ximages[i].reference = Some(r);
        }

        let r = self.ximage.ximages[i]
            .reference
            .expect("XObject without a reference");
        self.o.link(r)
    }
    /// `pdf_ximage_defineresource`: the id. `cdata` matches `subtype`.
    pub fn pdf_ximage_defineresource(
        &mut self,
        ident: Option<&[u8]>,
        subtype: i32,
        cdata: &XobjInfo,
        resource: Obj,
    ) -> i32 {
        let mut id: i32 = -1;
        let mut reserved = false;

        if let Some(ident) = ident {
            for (i, x) in self.ximage.ximages.iter().enumerate() {
                if x.ident.as_deref() == Some(ident) && x.reserved != 0 {
                    id = i as i32;
                    reserved = true;
                    break;
                }
            }
        }

        if !reserved {
            id = self.ximage.ximages.len() as i32;
            let mut x = PdfXimage::pdf_init_ximage_struct();

            if let Some(ident) = ident {
                x.ident = Some(ident.to_vec());
            }
            self.ximage.ximages.push(x);
        }

        match (subtype, cdata) {
            (PDF_XOBJECT_TYPE_IMAGE, XobjInfo::Image(info)) => {
                self.pdf_ximage_set_image(id, info, resource);
                self.ximage.ximages[id as usize].res_name = res_name(b"Im", id);
            }
            (PDF_XOBJECT_TYPE_FORM, XobjInfo::Form(info)) => {
                self.pdf_ximage_set_form(id, info, resource);
                self.ximage.ximages[id as usize].res_name = res_name(b"Fm", id);
            }
            _ => {
                error!("Unknown XObject subtype: {subtype}");
            }
        }

        id
    }
    /// `pdf_ximage_reserve`: the id of a reserved (forward-referenced)
    /// XObject.
    pub fn pdf_ximage_reserve(&mut self, ident: &[u8]) -> i32 {
        if self
            .ximage
            .ximages
            .iter()
            .any(|x| x.ident.as_deref() == Some(ident))
        {
            warn!("XObject ID already used!");
            return -1;
        }

        let id = self.ximage.ximages.len() as i32;

        let mut x = PdfXimage::pdf_init_ximage_struct();

        x.ident = Some(ident.to_vec());
        let mut names = self
            .doc
            .global_names
            .take()
            .expect("global_names not initialised");
        x.reference = self.o.pdf_names_reserve(&mut names, ident);
        self.doc.global_names = Some(names);
        x.res_name = res_name(b"Fm", id);
        x.reserved = 1;
        self.ximage.ximages.push(x);

        id
    }
    /// `pdf_ximage_get_resname`: a copy of the resource name.
    pub fn pdf_ximage_get_resname(&mut self, xobj_id: i32) -> Vec<u8> {
        self.ximage_check_id(xobj_id);

        self.ximage.ximages[xobj_id as usize].res_name.clone()
    }
    /// `pdf_ximage_get_subtype`.
    pub fn pdf_ximage_get_subtype(&mut self, xobj_id: i32) -> i32 {
        self.ximage_check_id(xobj_id);

        self.ximage.ximages[xobj_id as usize].subtype
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
        self.ximage_check_id(xobj_id);

        let a = &mut self.ximage.ximages[xobj_id as usize].attr;
        a.width = width;
        a.height = height;
        a.xdensity = xdensity;
        a.ydensity = ydensity;
        a.bbox.llx = llx;
        a.bbox.lly = lly;
        a.bbox.urx = urx;
        a.bbox.ury = ury;
    }
    /// `pdf_ximage_scale_image`: status, the matrix `M` and the clipping
    /// rectangle `r`, from the special's `p`.
    pub fn pdf_ximage_scale_image(
        &mut self,
        xobj_id: i32,
        p: &TransformInfo,
    ) -> (i32, PdfTmatrix, PdfRect) {
        self.ximage_check_id(xobj_id);

        let x = &self.ximage.ximages[xobj_id as usize];
        let mut m = PdfTmatrix {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: 0.0,
            f: 0.0,
        };
        let mut r = PdfRect::default();

        match x.subtype {
            /* See pdfximage.c's comment: images map the unit square,
             * densities give the size in bp.
             */
            PDF_XOBJECT_TYPE_IMAGE => {
                scale_to_fit_I(&mut m, p, x);
                if p.flags & INFO_HAS_USER_BBOX != 0 {
                    r.llx = p.bbox.llx / (f64::from(x.attr.width) * x.attr.xdensity);
                    r.lly = p.bbox.lly / (f64::from(x.attr.height) * x.attr.ydensity);
                    r.urx = p.bbox.urx / (f64::from(x.attr.width) * x.attr.xdensity);
                    r.ury = p.bbox.ury / (f64::from(x.attr.height) * x.attr.ydensity);
                } else {
                    r.llx = 0.0;
                    r.lly = 0.0;
                    r.urx = 1.0;
                    r.ury = 1.0;
                }
            }
            /* User-defined transformation and clipping are controlled by
             * the cm operator and W operator, explicitly */
            PDF_XOBJECT_TYPE_FORM => {
                scale_to_fit_F(&mut m, p, x);
                if p.flags & INFO_HAS_USER_BBOX != 0 {
                    r = p.bbox;
                } else {
                    /* I->attr.bbox from the image bounding box */
                    r = x.attr.bbox;
                }
            }
            _ => {
                /* maybe reserved */
                if p.flags & INFO_HAS_USER_BBOX != 0 {
                    r = p.bbox;
                } else {
                    r.llx = 0.0;
                    r.lly = 0.0;
                    r.urx = 1.0;
                    r.ury = 1.0;
                }
            }
        }

        (0, m, r)
    }
    /// `set_distiller_template`.
    pub fn set_distiller_template(&mut self, s: Option<&[u8]>) {
        self.ximage.cmdtmpl = match s {
            None | Some(b"") => None,
            Some(s) => Some(s.to_vec()),
        };
    }
    /// `get_distiller_template`: a copy.
    pub fn get_distiller_template(&mut self) -> Option<Vec<u8>> {
        self.ximage.cmdtmpl.clone()
    }
    /// `ps_include_page` (static): runs the distiller on a PS/EPS file.
    /// Without a distiller template it fails, as in C; running one (an
    /// external command and a temporary file) is not ported.
    pub fn ps_include_page(&mut self, xobj_id: i32, filename: &[u8], options: LoadOptions) -> i32 {
        let _ = (xobj_id, options);
        if self.ximage.cmdtmpl.is_none() {
            warn!(
                "No image converter available for converting file \"{}\" to PDF format.",
                String::from_utf8_lossy(filename)
            );
            warn!(">> Please check if you have 'D' option in config file.");
            return -1;
        }
        todo!("the distiller (an external command, dpx_file_apply_filter) is not ported")
    }
    /// `pdf_error_cleanup_cache`: deletes the temporary files (none here:
    /// the distiller is not ported).
    pub fn pdf_error_cleanup_cache(&mut self) {
        for x in &self.ximage.ximages {
            if x.attr.tempfile { /* dpx_delete_temp_file(I->fullname, false) */ }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;

    fn mem(d: &[u8]) -> MemFile {
        MemFile::new(Arc::from(d), b"t")
    }

    #[test]
    fn scale_image() {
        let mut x = PdfXimage::pdf_init_ximage_struct();
        x.attr.width = 100;
        x.attr.height = 50;
        let mut p = TransformInfo::default();
        p.flags = INFO_HAS_WIDTH;
        p.width = 200.0;
        let mut t = PdfTmatrix::default();
        scale_to_fit_I(&mut t, &p, &x);
        assert_eq!((t.a, t.d, t.e, t.f), (200.0, 100.0, 0.0, 0.0));
    }

    #[test]
    fn image_types() {
        assert_eq!(
            source_image_type(&mut mem(b"\xff\xd8\xff\xe0rest")),
            IMAGE_TYPE_JPEG
        );
        assert_eq!(
            source_image_type(&mut mem(b"%PDF-1.5\n%...")),
            IMAGE_TYPE_PDF
        );
        assert_eq!(
            source_image_type(&mut mem(b"BMxxxxxxxxxxxxxx")),
            IMAGE_TYPE_BMP
        );
        assert_eq!(
            source_image_type(&mut mem(b"%!PS-Adobe-3.0\n%%Creator: MetaPost 2.0\n")),
            IMAGE_TYPE_MPS
        );
        assert_eq!(
            source_image_type(&mut mem(b"%!PS-Adobe-3.0 EPSF\n")),
            IMAGE_TYPE_EPS
        );
        assert_eq!(
            source_image_type(&mut mem(b"hello world, nothing")),
            IMAGE_TYPE_UNKNOWN
        );
    }
}
