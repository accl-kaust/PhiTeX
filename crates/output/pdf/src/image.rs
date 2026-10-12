//! Images (`\pdfximage`, pdfTeX §1552–§1553, `writeimg.c`): reading an
//! image's size, scaling it (`scale_image`), placing it (`out_image`) and
//! writing its `XObject` (pdfTeX §1628): JPEG as `writejpg.c` (the file
//! as it is), PNG as `writepng.c` with libpng (`phitex_codecs::png`), a
//! page of a PDF file as `pdftoepdf.cc` (`phitex_codecs::pdfread`).

use std::sync::Arc;

use phitex_codecs::pdfread::{Dict, Doc, Obj, Ref, Stream};
use phitex_codecs::png::{self, Info, Transforms};
use phitex_typeset::arith::Scaled;
use phitex_typeset::node::RUNNING;

use crate::num::{ONE_HUNDRED_BP, ONE_HUNDRED_INCH, TEN_POW};
use crate::{FontSource, OBJ_TYPE_XIMAGE, PdfError, PdfState, int, pdf_error};

/// `one_inch`.
const ONE_INCH: Scaled = (ONE_HUNDRED_INCH + 50) / 100;

/// `image.h`'s `IMAGE_COLOR_*`: the `/ProcSet` an image needs.
pub(crate) const IMAGE_COLOR_B: i32 = 1;
pub(crate) const IMAGE_COLOR_C: i32 = 2;
pub(crate) const IMAGE_COLOR_I: i32 = 4;

const JPG_GRAY: u8 = 1;
const JPG_RGB: u8 = 3;
const JPG_CMYK: u8 = 4;

/// The page box of an included PDF page (`pdf_box_spec_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageBox {
    /// `mediabox`.
    Media = 1,
    /// `cropbox` (the default).
    Crop = 2,
    /// `bleedbox`.
    Bleed = 3,
    /// `trimbox`.
    Trim = 4,
    /// `artbox`.
    Art = 5,
}

/// What `\pdfximage` read: the rule specification (each
/// [`RUNNING`](phitex_typeset::node::RUNNING) when not given), `attr`,
/// the page (`page n` or `named{...}`), `colorspace n`, the page box
/// (after `\pdfpagebox` and its obsolete forms), and the file as
/// kpathsea found it.
#[derive(Clone, Debug)]
pub struct ImageSpec {
    /// Width.
    pub width: Scaled,
    /// Height.
    pub height: Scaled,
    /// Depth.
    pub depth: Scaled,
    /// `attr{...}`.
    pub attr: Option<Vec<u8>>,
    /// `page n` (1 if not given).
    pub page: i32,
    /// `named{...}`.
    pub named: Option<Vec<u8>>,
    /// `colorspace n` (0 if not given).
    pub colorspace: i32,
    /// The page box.
    pub page_box: PageBox,
    /// The file's name as found (`img_name`).
    pub name: Vec<u8>,
    /// The file's bytes.
    pub data: Arc<[u8]>,
    /// `\pdfimageresolution`.
    pub image_resolution: i32,
    /// `\pdfinclusionerrorlevel`.
    pub inclusion_errorlevel: i32,
}

/// What the TeX side reads of an image: its size, and
/// `\pdflastximagepages`, `\pdflastximagecolordepth`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct XImageInfo {
    /// Width.
    pub width: Scaled,
    /// Height.
    pub height: Scaled,
    /// Depth.
    pub depth: Scaled,
    /// `img_pages`.
    pub pages: i32,
    /// `img_colordepth`.
    pub colordepth: i32,
}

/// A PDF page's part of an image (`pdf_image_struct`).
#[derive(Clone, Debug)]
pub(crate) struct PdfImage {
    /// Index of its document in `epdf_docs`.
    doc: usize,
    page: i32,
    page_box: PageBox,
    orig_x: Scaled,
    orig_y: Scaled,
    rotate: i32,
    pages: i32,
}

#[derive(Clone, Debug)]
enum Kind {
    Jpg { color_space: u8, length: usize },
    Png,
    Pdf(PdfImage),
}

/// A read image (`image_entry`).
#[derive(Clone)]
struct Image {
    name: Vec<u8>,
    width: i32,
    height: i32,
    x_res: i32,
    y_res: i32,
    color: i32,
    colorspace_ref: i32,
    bits: i32,
    data: Arc<[u8]>,
    kind: Kind,
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Image")
            .field("name", &self.name)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("x_res", &self.x_res)
            .field("y_res", &self.y_res)
            .field("color", &self.color)
            .field("colorspace_ref", &self.colorspace_ref)
            .field("bits", &self.bits)
            .field("data", &crate::digest::Blob(&self.data))
            .field("kind", &self.kind)
            .finish()
    }
}

impl Image {
    fn rotate(&self) -> i32 {
        match &self.kind {
            Kind::Pdf(p) => p.rotate,
            _ => 0,
        }
    }

    /// `img_width`, `img_height` as the page is shown.
    fn shown(&self) -> (i32, i32) {
        if matches!(self.rotate(), 90 | 270) {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }
}

/// `\pdfximage`'s data (`obj_ximage_*`).
#[derive(Clone, Debug)]
pub(crate) struct XImage {
    width: Scaled,
    height: Scaled,
    depth: Scaled,
    attr: Option<Vec<u8>>,
    image: Option<Image>,
    pages: i32,
    bits: i32,
    pub color: i32,
    /// `img_group_ref`: -1 a PDF page's group to be numbered, a PNG's
    /// object, or 0.
    group_ref: i32,
}

/// An object of an included file copied (`InObj`).
#[derive(Clone, Debug)]
struct InObj {
    r: Ref,
    objnum: usize,
    written: bool,
    kind: InKind,
}

/// `InObjType`.
#[derive(Clone, Debug)]
enum InKind {
    Other,
    /// `objFont`: a replaced font, its descriptor's key and its encoding.
    Font(crate::epdffont::FdKey, usize),
    /// `objFontDesc`: written by the font descriptors.
    FontDesc,
}

/// An open document (`PdfDocument`).
#[derive(Clone, Debug)]
pub(crate) struct EpdfDoc {
    name: Vec<u8>,
    /// `occurences`.
    occurrences: i32,
    objs: Vec<InObj>,
    open: bool,
}

/// `ext_xn_over_d` (`utils.c`).
#[allow(clippy::cast_possible_truncation)]
fn ext_xn_over_d(x: Scaled, n: i64, d: i64) -> Scaled {
    #[allow(clippy::cast_precision_loss)]
    let mut r = (f64::from(x) * n as f64) / d as f64;
    if r > f64::EPSILON {
        r += 0.5;
    } else {
        r -= 0.5;
    }
    r as Scaled
}

/// `bp2int`: big points in a `float`, as scaled points (C's `round`).
#[allow(clippy::cast_possible_truncation)]
fn bp2int(p: f32) -> i32 {
    (f64::from(p) * (f64::from(ONE_HUNDRED_BP) / 100.0)).round() as i32
}

/// `convertNumToPDF`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn num_to_pdf(n: f64) -> Vec<u8> {
    const EPSILON: f64 = 0.5e-6;
    if n.abs() < EPSILON {
        return b"0".to_vec();
    }
    let mut out = Vec::new();
    let mut n = n;
    if n < 0.0 {
        out.push(b'-');
        n = -n;
    }
    n += EPSILON;
    let ival = n as i32;
    n -= f64::from(ival);
    out.extend_from_slice(ival.to_string().as_bytes());
    let mut fval = (n * 1e6) as i32;
    if fval != 0 {
        let mut digits = [0u8; 6];
        for d in digits.iter_mut().rev() {
            *d = b'0' + (fval % 10) as u8;
            fval /= 10;
        }
        let keep = 6 - digits.iter().rev().take_while(|&&d| d == b'0').count();
        out.push(b'.');
        out.extend_from_slice(&digits[..keep]);
    }
    out
}

/// `stripzeros` (`utils.c`).
fn stripzeros(a: &[u8]) -> Vec<u8> {
    #[derive(Clone, Copy, PartialEq)]
    enum S {
        NoNum,
        DotNoNum,
        Int,
        Dot,
        LeadDot,
        Frac,
    }
    let mut buf = a.to_vec();
    let (mut s, mut t) = (S::NoNum, S::NoNum);
    let (mut p, mut q, mut r) = (0usize, 0usize, 0usize);
    let digit = |c: u8| c.is_ascii_digit();
    while p < buf.len() {
        let c = buf[p];
        s = match s {
            S::NoNum if digit(c) => S::Int,
            S::NoNum if c == b'.' => S::LeadDot,
            S::Int if c == b'.' => S::Dot,
            S::Int if digit(c) => S::Int,
            S::Dot | S::LeadDot | S::Frac if digit(c) => S::Frac,
            S::Dot | S::LeadDot | S::Frac | S::DotNoNum if c == b'.' => S::DotNoNum,
            S::DotNoNum if digit(c) => S::DotNoNum,
            _ => S::NoNum,
        };
        match s {
            S::Dot => r = q,
            S::LeadDot => r = q + 1,
            S::Frac if c > b'0' => r = q + 1,
            S::NoNum if (t == S::Frac || t == S::Dot) && r != 0 => {
                q = r;
                r -= 1;
                if buf[r] == b'.' {
                    buf[r] = b'0';
                }
                r = 0;
            }
            _ => {}
        }
        buf[q] = buf[p];
        q += 1;
        p += 1;
        t = s;
    }
    buf.truncate(q);
    buf
}

/// `sprintf("%.8f")` of each, separated by spaces.
fn eight(xs: &[f64]) -> Vec<u8> {
    let v: Vec<String> = xs.iter().map(|x| format!("{x:.8}")).collect();
    v.join(" ").into_bytes()
}

/// `convertStringToPDFString`.
fn pdf_string(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for &c in s {
        if !(b'!'..=b'~').contains(&c) {
            out.extend_from_slice(format!("\\{c:03o}").as_bytes());
        } else if c == b'(' || c == b')' || c == b'\\' {
            out.push(b'\\');
            out.push(c);
        } else {
            out.push(c);
        }
    }
    out
}

fn page_box(p: &phitex_codecs::pdfread::Page, spec: PageBox) -> [f64; 4] {
    match spec {
        PageBox::Media => p.media,
        PageBox::Crop => p.crop,
        PageBox::Bleed => p.bleed,
        PageBox::Trim => p.trim,
        PageBox::Art => p.art,
    }
}

fn fail(name: &[u8], msg: &str) -> PdfError {
    pdf_error("", &format!("{}: {msg}", String::from_utf8_lossy(name)))
}

/// `read_APP1_Exif`: the resolution.
#[allow(clippy::cast_possible_truncation)]
fn read_exif(buf: &[u8]) -> (i32, i32) {
    let at = |i: usize| buf.get(i).copied().unwrap_or(0);
    let bytes = |p: &mut usize, n: usize, big: bool| -> u32 {
        let mut v: u32 = 0;
        if big {
            for k in 0..n {
                v = (v << 8) + u32::from(at(*p + k));
            }
        } else {
            for k in (0..n).rev() {
                v = (v << 8) + u32::from(at(*p + k));
            }
        }
        *p += n;
        v
    };
    let mut p = 0;
    while p < buf.len() && buf[p] == 0 {
        p += 1;
    }
    let tiff = p;
    let big = match (at(p), at(p + 1)) {
        (b'M', b'M') => true,
        (b'I', b'I') => false,
        _ => return (0, 0),
    };
    p += 2;
    if bytes(&mut p, 2, big) != 42 {
        return (0, 0);
    }
    let i = bytes(&mut p, 4, big) as usize;
    p = tiff + i;
    let mut num_fields = i64::from(bytes(&mut p, 2, big));
    let (mut xres, mut yres, mut res_unit) = (72.0f64, 72.0f64, 1.0f64);
    let (mut value, mut num, mut den) = (0u32, 0u32, 0u32);
    while num_fields > 0 {
        num_fields -= 1;
        let tag = bytes(&mut p, 2, big);
        let ty = bytes(&mut p, 2, big);
        bytes(&mut p, 4, big);
        match ty {
            1 | 7 => {
                value = u32::from(at(p));
                p += 4;
            }
            3 => {
                value = bytes(&mut p, 2, big);
                p += 2;
            }
            4 | 9 => value = bytes(&mut p, 4, big),
            5 | 10 => {
                value = bytes(&mut p, 4, big);
                let mut rp = tiff + value as usize;
                num = bytes(&mut rp, 4, big);
                den = bytes(&mut rp, 4, big);
            }
            _ => p += 4,
        }
        match tag {
            // `num / den`: C's int division
            282 if den != 0 => xres = f64::from(num.cast_signed() / den.cast_signed()),
            283 if den != 0 => yres = f64::from(num.cast_signed() / den.cast_signed()),
            296 => match value {
                2 => res_unit = 1.0,
                3 => res_unit = 2.54,
                _ => {}
            },
            _ => {}
        }
    }
    ((xres * res_unit) as i32, (yres * res_unit) as i32)
}

/// `read_jpg_info`.
#[allow(clippy::cast_possible_truncation)]
fn read_jpg(name: &[u8], data: &Arc<[u8]>, pdf12: bool) -> Result<Image, PdfError> {
    let d = &data[..];
    let two = |i: usize| -> u16 {
        u16::from(d.get(i).copied().unwrap_or(0)) << 8
            | u16::from(d.get(i + 1).copied().unwrap_or(0))
    };
    if two(0) != 0xFFD8 {
        return Err(fail(
            name,
            "reading JPEG image failed (no JPEG header found)",
        ));
    }
    let (mut x_res, mut y_res) = (0i32, 0i32);
    match two(2) {
        0xFFE0 => {
            if d.get(6..11) == Some(b"JFIF\0") {
                let units = d.get(13).copied().unwrap_or(0);
                x_res = i32::from(two(14));
                y_res = i32::from(two(16));
                match units {
                    1 => {}
                    2 => {
                        x_res = (f64::from(x_res) * 2.54) as i32;
                        y_res = (f64::from(y_res) * 2.54) as i32;
                    }
                    _ => (x_res, y_res) = (0, 0),
                }
            }
            if x_res == 0 && y_res != 0 {
                x_res = y_res;
            }
            if y_res == 0 && x_res != 0 {
                y_res = x_res;
            }
        }
        0xFFE1 => {
            let length = two(4).wrapping_sub(2);
            if length > 5 && d.get(6..11) == Some(b"Exif\0") {
                let n = usize::from(length - 5);
                let buf = d.get(11..(11 + n).min(d.len())).unwrap_or_default();
                let mut b = buf.to_vec();
                b.resize(n, 0);
                (x_res, y_res) = read_exif(&b);
            }
        }
        _ => {}
    }
    let mut i = 0;
    loop {
        if i >= d.len() {
            return Err(fail(name, "reading JPEG image failed (premature file end)"));
        }
        if d[i] != 0xFF {
            return Err(fail(name, "reading JPEG image failed (no marker found)"));
        }
        let m = d.get(i + 1).copied().unwrap_or(0);
        i += 2;
        match m {
            0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                return Err(fail(name, "unsupported type of compression"));
            }
            0xC0..=0xC3 => {
                if m == 0xC2 && pdf12 {
                    return Err(fail(name, "cannot use progressive DCT with PDF-1.2"));
                }
                let bits = i32::from(d.get(i + 2).copied().unwrap_or(0));
                let height = i32::from(two(i + 3));
                let width = i32::from(two(i + 5));
                let color_space = d.get(i + 7).copied().unwrap_or(0);
                let color = match color_space {
                    JPG_GRAY => IMAGE_COLOR_B,
                    JPG_RGB | JPG_CMYK => IMAGE_COLOR_C,
                    c => return Err(fail(name, &format!("Unsupported color space {c}"))),
                };
                return Ok(Image {
                    name: name.to_vec(),
                    width,
                    height,
                    x_res,
                    y_res,
                    color,
                    colorspace_ref: 0,
                    bits,
                    data: data.clone(),
                    kind: Kind::Jpg {
                        color_space,
                        length: d.len(),
                    },
                });
            }
            0xD8 | 0xD9 | 0x01 | 0xD0..=0xD7 => {}
            _ => i += usize::from(two(i)),
        }
    }
}

/// `spng_getint`.
fn getint(d: &[u8], pos: usize) -> Option<i32> {
    let b = d.get(pos..pos + 4)?;
    Some(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

const IDAT: i32 = 0x4944_4154;
const IEND: i32 = 0x4945_4E44;

/// A copy of an included page in progress (`write_epdf`'s locals and
/// the document's `inObjList`).
struct Copy<'d> {
    doc: &'d Doc,
    objs: Vec<InObj>,
    name: Vec<u8>,
    /// `encodingList`: the encodings of replaced fonts, newest last.
    encs: Vec<(usize, crate::epdffont::Names)>,
    src: &'d dyn FontSource,
}

impl PdfState {
    // ---- reading (writeimg.c) ----

    /// `\pdfximage` (pdfTeX §1552): the image's object number
    /// (`\pdflastximage`); the file is read now.
    ///
    /// # Errors
    /// What `read_image` and `scale_image` stop at (`pdftex_fail`,
    /// `pdf_error`), or an image this crate does not read (JBIG2).
    pub fn ximage(&mut self, spec: ImageSpec) -> Result<usize, PdfError> {
        self.check_pdfversion();
        self.ximage_count += 1;
        let k = self.create_obj(OBJ_TYPE_XIMAGE, self.ximage_count);
        let (mut image, group_ref) = self.read_image(&spec)?;
        image.colorspace_ref = spec.colorspace;
        let pages = match &image.kind {
            Kind::Pdf(p) => p.pages,
            _ => 1,
        };
        let mut x = XImage {
            width: spec.width,
            height: spec.height,
            depth: spec.depth,
            attr: spec.attr,
            pages,
            bits: image.bits,
            color: image.color,
            group_ref,
            image: None,
        };
        scale_image(&mut x, &image, spec.image_resolution)?;
        x.image = Some(image);
        self.aux.ximages.insert(k, x);
        Ok(k)
    }

    /// `read_image`: the type by the first bytes or the name.
    fn read_image(&mut self, spec: &ImageSpec) -> Result<(Image, i32), PdfError> {
        let d = &spec.data;
        let name = &spec.name;
        if d.len() < 8 {
            return Err(fail(name, "reading image file failed"));
        }
        let suffix = name
            .iter()
            .rposition(|&c| c == b'.')
            .map(|i| name[i..].to_ascii_lowercase());
        let kind: &[u8] = if d.starts_with(b"\xFF\xD8") {
            b"jpg"
        } else if d.starts_with(b"\x89PNG\r\n\x1A\n") {
            b"png"
        } else if d.starts_with(b"\x97JB2\r\n\x1A\n") {
            b"jb2"
        } else if d.starts_with(b"%PDF-1.") {
            b"pdf"
        } else {
            match suffix.as_deref() {
                Some(b".png") => b"png",
                Some(b".jpg" | b".jpeg") => b"jpg",
                Some(b".jbig2" | b".jb2") => b"jb2",
                Some(b".pdf") => b"pdf",
                _ => return Err(fail(name, "unknown type of image")),
            }
        };
        match kind {
            b"pdf" => self.read_pdf(spec),
            b"jpg" => {
                let pdf12 = self.fixed_major == 1 && self.fixed_minor <= 2;
                Ok((read_jpg(name, d, pdf12)?, 0))
            }
            b"png" => self.read_png(name, d),
            _ => Err(PdfError::Unsupported("JBIG2 images".into())),
        }
    }

    /// `read_png_info`.
    #[allow(clippy::cast_possible_truncation)]
    fn read_png(&mut self, name: &[u8], data: &Arc<[u8]>) -> Result<(Image, i32), PdfError> {
        let info = png::read_info(data).map_err(|_| fail(name, "libpng: internal error"))?;
        let (mut x_res, mut y_res) = (0, 0);
        if info.get_valid(png::INFO_PHYS) != 0 {
            let (x, y, unit) = info.phys.unwrap_or((0, 0, 0));
            let per_meter = |v: u32| if unit == 1 { f64::from(v) } else { 0.0 };
            x_res = (0.0254 * per_meter(x)).round() as i32;
            y_res = (0.0254 * per_meter(y)).round() as i32;
        }
        let color = match info.color_type {
            png::COLOR_PALETTE => IMAGE_COLOR_C | IMAGE_COLOR_I,
            png::COLOR_GRAY | png::COLOR_GRAY_ALPHA => IMAGE_COLOR_B,
            png::COLOR_RGB | png::COLOR_RGB_ALPHA => IMAGE_COLOR_C,
            t => return Err(fail(name, &format!("unsupported type of color_type <{t}>"))),
        };
        let mut group_ref = 0;
        if (self.fixed_major > 1 || self.fixed_minor >= 4)
            && matches!(
                info.color_type,
                png::COLOR_GRAY_ALPHA | png::COLOR_RGB_ALPHA
            )
        {
            if self.png_group == 0 {
                self.png_group = self.new_objnum_();
            }
            if self.page_group_val == 0 {
                self.page_group_val = self.png_group;
            }
            group_ref = i32::try_from(self.page_group_val).unwrap_or(0);
        }
        Ok((
            Image {
                name: name.to_vec(),
                width: i32::try_from(info.width).unwrap_or(i32::MAX),
                height: i32::try_from(info.height).unwrap_or(i32::MAX),
                x_res,
                y_res,
                color,
                colorspace_ref: 0,
                bits: i32::from(info.bit_depth),
                data: data.clone(),
                kind: Kind::Png,
            },
            group_ref,
        ))
    }

    /// `read_pdf_info` (`pdftoepdf.cc`).
    #[allow(clippy::cast_possible_truncation, clippy::too_many_lines)]
    fn read_pdf(&mut self, spec: &ImageSpec) -> Result<(Image, i32), PdfError> {
        let name = &spec.name;
        // find_add_document
        let doc_ix = if let Some(i) = self
            .epdf_docs
            .iter()
            .position(|d| d.open && d.name == *name)
        {
            self.epdf_docs[i].occurrences += 1;
            i
        } else {
            self.epdf_docs.push(EpdfDoc {
                name: name.clone(),
                occurrences: 0,
                objs: Vec::new(),
                open: true,
            });
            self.epdf_docs.len() - 1
        };
        let doc =
            Doc::open(&spec.data).map_err(|_| fail(name, "xpdf: reading PDF image failed"))?;
        let found = doc.version as f32;
        let wanted = (f64::from(self.fixed_major) + f64::from(self.fixed_minor) * 0.1) as f32;
        if f64::from(found) > f64::from(wanted) + 0.01 && spec.inclusion_errorlevel > 0 {
            return Err(fail(
                name,
                &format!(
                    "PDF inclusion: found PDF version <{:.1}>, but at most version <{:.1}> allowed",
                    f64::from(found),
                    f64::from(wanted)
                ),
            ));
        }
        let pages = i32::try_from(doc.num_pages()).unwrap_or(i32::MAX);
        let page_num = if let Some(dest) = &spec.named {
            let Some(n) = doc.find_dest(dest) else {
                return Err(fail(
                    name,
                    &format!(
                        "PDF inclusion: invalid destination <{}>",
                        String::from_utf8_lossy(dest)
                    ),
                ));
            };
            if n == 0 {
                return Err(fail(
                    name,
                    &format!(
                        "PDF inclusion: destination is not a page <{}>",
                        String::from_utf8_lossy(dest)
                    ),
                ));
            }
            i32::try_from(n).unwrap_or(i32::MAX)
        } else {
            if spec.page <= 0 || spec.page > pages {
                return Err(fail(
                    name,
                    &format!("PDF inclusion: required page does not exist <{pages}>"),
                ));
            }
            spec.page
        };
        let page = doc
            .page(usize::try_from(page_num).unwrap_or(0))
            .ok_or_else(|| fail(name, "xpdf: reading PDF image failed"))?;
        let b = page_box(&page, spec.page_box);
        let f = |x: f64| x as f32;
        let (orig_x, width) = if b[2] > b[0] {
            (f(b[0]), f(b[2] - b[0]))
        } else {
            (f(b[2]), f(b[0] - b[2]))
        };
        let (orig_y, height) = if b[3] > b[1] {
            (f(b[1]), f(b[3] - b[1]))
        } else {
            (f(b[3]), f(b[1] - b[3]))
        };
        let group = matches!(
            doc.fetch(page.dict_ref)
                .as_dict()
                .map(|d| doc.lookup(d, b"Group")),
            Some(Obj::Dict(_))
        );
        Ok((
            Image {
                name: name.clone(),
                width: bp2int(width),
                height: bp2int(height),
                x_res: 0,
                y_res: 0,
                color: 0,
                colorspace_ref: 0,
                bits: 0,
                data: spec.data.clone(),
                kind: Kind::Pdf(PdfImage {
                    doc: doc_ix,
                    page: page_num,
                    page_box: spec.page_box,
                    orig_x: bp2int(orig_x),
                    orig_y: bp2int(orig_y),
                    rotate: page.rotate,
                    pages,
                }),
            },
            if group { -1 } else { 0 },
        ))
    }

    /// Image `n`'s size and numbers.
    #[must_use]
    pub fn ximage_info(&self, n: usize) -> Option<XImageInfo> {
        self.aux.ximages.get(&n).map(|x| XImageInfo {
            width: x.width,
            height: x.height,
            depth: x.depth,
            pages: x.pages,
            colordepth: x.bits,
        })
    }

    /// Whether `n` is a `\pdfximage` (`pdf_check_obj(obj_type_ximage, n)`).
    #[must_use]
    pub fn is_ximage(&self, n: usize) -> bool {
        self.in_list(OBJ_TYPE_XIMAGE, n)
    }

    /// `\pdfximagebbox n j` (1 to 4): the page box of an included PDF
    /// page, 0 for other images.
    #[must_use]
    pub fn ximage_bbox(&self, n: usize, j: i32) -> Scaled {
        let Some(i) = self.aux.ximages.get(&n).and_then(|x| x.image.as_ref()) else {
            return 0;
        };
        match &i.kind {
            Kind::Pdf(p) => match j {
                1 => p.orig_x,
                2 => p.orig_y,
                3 => p.orig_x + i.width,
                _ => p.orig_y + i.height,
            },
            _ => 0,
        }
    }

    /// `\immediate\pdfximage`: `pdf_write_image(pdf_last_ximage)`.
    ///
    /// # Errors
    /// What writing the image meets.
    pub fn write_ximage(&mut self, n: usize, src: &dyn FontSource) -> Result<(), PdfError> {
        self.pdf_write_image(n, src)
    }

    // ---- on the page ----

    /// `out_image` (pdfTeX §1620).
    pub(crate) fn out_image(&mut self, n: usize, width: Scaled, height: Scaled, depth: Scaled) {
        let Some(x) = self.aux.ximages.get(&n) else {
            return;
        };
        let Some(image) = x.image.clone() else {
            return;
        };
        let group_ref = x.group_ref;
        let (img_w, img_h) = image.shown();
        self.end_text();
        self.print_ln(b"q");
        if !self.ximage_list.contains(&n) {
            self.ximage_list.push(n);
        }
        let px = self.cur_h - self.origin_h;
        let py = self.origin_v - self.cur_v;
        let total = height + depth;
        if let Kind::Pdf(p) = &image.kind {
            if group_ref != 0 && self.page_group_val == 0 {
                if group_ref == -1 {
                    self.page_group_val = self.new_objnum_();
                    let g = i32::try_from(self.page_group_val).unwrap_or(0);
                    if let Some(x) = self.aux.ximages.get_mut(&n) {
                        x.group_ref = g;
                    }
                } else {
                    self.page_group_val = usize::try_from(group_ref).unwrap_or(0);
                }
            }
            self.print_real(ext_xn_over_d(width, TEN_POW[6], i64::from(img_w)), 6);
            self.print(b" 0 0 ");
            self.print_real(ext_xn_over_d(total, TEN_POW[6], i64::from(img_h)), 6);
            self.pdf_out(b' ');
            self.print_bp(px - ext_xn_over_d(width, i64::from(p.orig_x), i64::from(img_w)));
            self.pdf_out(b' ');
            self.print_bp(py - ext_xn_over_d(total, i64::from(p.orig_y), i64::from(img_h)));
        } else {
            if matches!(image.kind, Kind::Png) && group_ref > 0 && self.page_group_val == 0 {
                self.page_group_val = usize::try_from(group_ref).unwrap_or(0);
            }
            let hb = i64::from(ONE_HUNDRED_BP);
            self.print_real(ext_xn_over_d(width, TEN_POW[6], hb), 4);
            self.print(b" 0 0 ");
            self.print_real(ext_xn_over_d(total, TEN_POW[6], hb), 4);
            self.pdf_out(b' ');
            self.print_bp(px);
            self.pdf_out(b' ');
            self.print_bp(py);
        }
        self.print_ln(b" cm");
        self.print(b"/Im");
        self.print_int(self.objs[n].info);
        self.print_ln(b" Do");
        self.print_ln(b"Q");
    }

    // ---- writing ----

    /// `pdf_write_image` (pdfTeX §1628) with `write_image`.
    pub(crate) fn pdf_write_image(
        &mut self,
        n: usize,
        src: &dyn FontSource,
    ) -> Result<(), PdfError> {
        let Some(x) = self.aux.ximages.get_mut(&n) else {
            return Ok(());
        };
        let attr = x.attr.take();
        let image = x.image.clone();
        self.begin_dict(n, 0);
        if let Some(a) = &attr {
            self.print_toks_ln(a);
        }
        let Some(image) = image else {
            return Ok(());
        };
        if self.fixed_draftmode != 0 {
            return Ok(());
        }
        match &image.kind {
            Kind::Pdf(p) => {
                let r = self.write_epdf(&image, p, src);
                // delete_image: the document closed when no image of it
                // is left to write
                let d = &mut self.epdf_docs[p.doc];
                if d.occurrences < 0 {
                    d.open = false;
                    d.objs.clear();
                }
                r
            }
            Kind::Png => self.write_png(&image),
            Kind::Jpg {
                color_space,
                length,
            } => {
                self.print(b"/Type /XObject\n/Subtype /Image\n");
                self.print(
                    format!(
                        "/Width {}\n/Height {}\n/BitsPerComponent {}\n/Length {}\n",
                        image.width, image.height, image.bits, length
                    )
                    .as_bytes(),
                );
                self.print(b"/ColorSpace ");
                if image.colorspace_ref != 0 {
                    self.print(format!("{} 0 R\n", image.colorspace_ref).as_bytes());
                } else {
                    match *color_space {
                        JPG_GRAY => self.print(b"/DeviceGray\n"),
                        JPG_RGB => self.print(b"/DeviceRGB\n"),
                        _ => self.print(b"/DeviceCMYK\n/Decode [1 0 1 0 1 0 1 0]\n"),
                    }
                }
                self.print(b"/Filter /DCTDecode\n>>\nstream\n");
                self.print(&image.data);
                self.end_raw_stream();
                Ok(())
            }
        }
    }

    /// `pdf_printf("%i 0 R\n")` of a colour space, or the device's.
    fn png_device(&mut self, colorspace_ref: i32, gray: bool) {
        if colorspace_ref != 0 {
            self.print(format!("{colorspace_ref} 0 R\n").as_bytes());
        } else if gray {
            self.print(b"/DeviceGray\n");
        } else {
            self.print(b"/DeviceRGB\n");
        }
    }

    fn png_stream(&mut self, bytes: &[u8]) {
        self.begin_stream();
        self.print(bytes);
        self.end_stream();
    }

    /// `write_png`.
    #[allow(clippy::too_many_lines)]
    fn write_png(&mut self, image: &Image) -> Result<(), PdfError> {
        let name = &image.name;
        let info = png::read_info(&image.data).map_err(|_| fail(name, "libpng: internal error"))?;
        let (major, minor) = (self.fixed_major, self.fixed_minor);
        if major == 1 && minor < 5 {
            self.fixed_image_hicolor = 0;
        }
        self.print(b"/Type /XObject\n/Subtype /Image\n");
        let mut copy = true;
        let mut t = Transforms::default();
        if info.get_valid(png::INFO_TRNS) != 0 {
            t.trns_to_alpha = true;
            copy = false;
        }
        if major == 1 && minor < 4 && info.color_type & 4 != 0 {
            t.strip_alpha = true;
            copy = false;
        }
        if info.bit_depth == 16 && self.fixed_image_hicolor == 0 {
            t.strip_16 = true;
            copy = false;
        }
        let file_gamma = (info.get_valid(png::INFO_GAMA) != 0).then_some(info.gamma);
        if self.p.image_apply_gamma != 0 {
            return Err(PdfError::Unsupported("\\pdfimageapplygamma".into()));
        }
        let (color_type, bit_depth) = png::output(&info, t);
        self.print(
            format!(
                "/Width {}\n/Height {}\n/BitsPerComponent {}\n",
                info.width, info.height, bit_depth
            )
            .as_bytes(),
        );
        self.print(b"/ColorSpace ");
        let others = png::INFO_CHRM
            | png::INFO_ICCP
            | png::INFO_SBIT
            | png::INFO_SRGB
            | png::INFO_BKGD
            | png::INFO_HIST
            | png::INFO_TRNS
            | png::INFO_SPLT;
        if copy
            && (major > 1 || minor > 1)
            && info.interlace == 0
            && matches!(color_type, png::COLOR_GRAY | png::COLOR_RGB)
            && file_gamma.is_none_or(|g| g == 100_000)
            && info.get_valid(others) == 0
        {
            self.png_device(image.colorspace_ref, color_type == png::COLOR_GRAY);
            return self.copy_png(image, &info, bit_depth);
        }
        let img = png::read_image(&image.data, &info, t)
            .map_err(|_| fail(name, "libpng: internal error"))?;
        let rows = &img.rows;
        let mut needs_group = false;
        match img.color_type {
            png::COLOR_PALETTE => {
                let palette_objnum = self.new_objnum_();
                if image.colorspace_ref != 0 {
                    self.print(format!("{} 0 R\n", image.colorspace_ref).as_bytes());
                } else {
                    self.print(
                        format!(
                            "[/Indexed /DeviceRGB {} {palette_objnum} 0 R]\n",
                            int(info.palette.len()) - 1
                        )
                        .as_bytes(),
                    );
                }
                self.png_stream(rows);
                self.begin_dict(palette_objnum, 0);
                let bytes: Vec<u8> = info.palette.iter().flatten().copied().collect();
                self.png_stream(&bytes);
            }
            png::COLOR_GRAY | png::COLOR_RGB => {
                self.png_device(image.colorspace_ref, img.color_type == png::COLOR_GRAY);
                self.png_stream(rows);
            }
            png::COLOR_GRAY_ALPHA | png::COLOR_RGB_ALPHA if major > 1 || minor >= 4 => {
                let gray = img.color_type == png::COLOR_GRAY_ALPHA;
                self.png_device(image.colorspace_ref, gray);
                let smask_objnum = self.new_objnum_();
                self.print(format!("/SMask {smask_objnum} 0 R\n").as_bytes());
                let wide = img.bit_depth == 16 && self.fixed_image_hicolor != 0;
                let (n, alpha_from) = match (gray, wide) {
                    (true, true) => (4, 2),
                    (true, false) => (2, 1),
                    (false, true) => (8, 6),
                    (false, false) => (4, 3),
                };
                let mut color = Vec::with_capacity(rows.len());
                let mut smask = Vec::new();
                for row in rows.chunks(img.rowbytes.max(1)) {
                    for piece in row.chunks(16384) {
                        for (j, &b) in piece.iter().enumerate() {
                            if j % n < alpha_from {
                                color.push(b);
                            } else {
                                smask.push(b);
                            }
                        }
                    }
                }
                self.png_stream(&color);
                let depth = img.bit_depth;
                self.begin_dict(smask_objnum, 0);
                self.print(b"/Type /XObject\n/Subtype /Image\n");
                self.print(
                    format!(
                        "/Width {}\n/Height {}\n/BitsPerComponent {}\n",
                        info.width,
                        info.height,
                        if depth == 16 { 8 } else { depth }
                    )
                    .as_bytes(),
                );
                self.print(b"/ColorSpace /DeviceGray\n");
                let mask: Vec<u8> = if depth == 16 {
                    smask.iter().step_by(2).copied().collect()
                } else {
                    smask
                };
                self.png_stream(&mask);
                needs_group = true;
            }
            png::COLOR_GRAY_ALPHA | png::COLOR_RGB_ALPHA => {
                self.png_device(
                    image.colorspace_ref,
                    img.color_type == png::COLOR_GRAY_ALPHA,
                );
                self.png_stream(rows);
            }
            t => return Err(fail(name, &format!("unsupported type of color_type <{t}>"))),
        }
        // write_additional_png_objects
        if needs_group && !self.png_group_written && self.png_group > 0 {
            self.png_group_written = true;
            let g = self.png_group;
            self.begin_obj(g, 2);
            if self.p.compress_level == 0 {
                self.print(b"%PTEX Group needed for transparent pngs\n");
            }
            self.print(b"<</Type/Group /S/Transparency /CS/DeviceRGB /I true>>\n");
            self.end_obj();
        }
        Ok(())
    }

    /// `copy_png`: the IDAT data as they are.
    fn copy_png(&mut self, image: &Image, info: &Info, depth: u8) -> Result<(), PdfError> {
        let d = &image.data[..];
        let name = &image.name;
        let broken = || fail(name, "writepng: reading chunk type failed");
        let mut pos = 8usize;
        let mut length: i32 = 0;
        loop {
            let (Some(len), Some(kind)) = (getint(d, pos), getint(d, pos + 4)) else {
                return Err(broken());
            };
            pos += 8;
            if kind == IEND {
                break;
            }
            if kind == IDAT {
                length = length.wrapping_add(len);
            }
            pos = pos.saturating_add(usize::try_from(len).unwrap_or(0) + 4);
        }
        let colors = if info.color_type == png::COLOR_RGB {
            3
        } else {
            1
        };
        self.print(
            format!(
                "/Length {length}\n/Filter/FlateDecode\n/DecodeParms<</Colors {colors}/Columns {}/BitsPerComponent {depth}/Predictor 10>>\n>>\nstream\n",
                info.width
            )
            .as_bytes(),
        );
        let mut pos = 8usize;
        let mut idat = 0;
        loop {
            let (Some(len), Some(kind)) = (getint(d, pos), getint(d, pos + 4)) else {
                return Err(broken());
            };
            pos += 8;
            let n = usize::try_from(len).unwrap_or(0);
            match kind {
                IDAT => {
                    if idat == 2 {
                        return Err(fail(name, "writepng: IDAT chunk sequence broken"));
                    }
                    idat = 1;
                    let end = (pos + n).min(d.len());
                    self.print(&d[pos.min(end)..end]);
                    pos += n + 4;
                }
                IEND => {
                    self.end_raw_stream();
                    return Ok(());
                }
                _ => {
                    if idat == 1 {
                        idat = 2;
                    }
                    pos = pos.saturating_add(n + 4);
                }
            }
        }
    }

    // ---- pdftoepdf.cc ----

    /// `pdf_puts`: bytes, which set `pdf_last_byte`.
    fn ep_puts(&mut self, s: &[u8]) {
        self.print(s);
        if let Some(&c) = s.last() {
            self.last_byte = c;
        }
    }

    /// `pdf_newline`.
    fn ep_newline(&mut self) {
        if self.last_byte != b'\n' {
            self.ep_puts(b"\n");
        }
    }

    /// `copyName`.
    fn ep_name(&mut self, s: &[u8]) {
        self.ep_puts(b"/");
        for &c in s {
            if c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-' | b'+') {
                self.pdf_out(c);
            } else {
                self.ep_puts(format!("#{c:02X}").as_bytes());
            }
        }
    }

    /// `addOther`.
    fn ep_add(&mut self, c: &mut Copy<'_>, r: Ref) -> Result<usize, PdfError> {
        if r.num == 0 {
            return Err(fail(&c.name, "PDF inclusion: invalid reference"));
        }
        if let Some(o) = c.objs.iter().find(|o| o.r == r) {
            return Ok(o.objnum);
        }
        let objnum = self.new_objnum_();
        c.objs.push(InObj {
            r,
            objnum,
            written: false,
            kind: InKind::Other,
        });
        Ok(objnum)
    }

    fn ep_dict(&mut self, c: &mut Copy<'_>, d: &Dict) -> Result<(), PdfError> {
        for (k, v) in &d.0 {
            self.ep_name(k);
            self.ep_puts(b" ");
            self.ep_object(c, v)?;
            self.ep_puts(b"\n");
        }
        Ok(())
    }

    /// `copyStream`: bytes as they are.
    fn ep_stream_bytes(&mut self, data: &[u8]) {
        self.print(data);
        self.last_byte = data.last().copied().unwrap_or(0);
    }

    /// `copyObject`.
    fn ep_object(&mut self, c: &mut Copy<'_>, o: &Obj) -> Result<(), PdfError> {
        match o {
            Obj::Bool(b) => self.ep_puts(if *b { b"true" } else { b"false" }),
            Obj::Int(i) => self.ep_puts(i.to_string().as_bytes()),
            Obj::Real(r) => self.ep_puts(&num_to_pdf(*r)),
            Obj::Str(s) => {
                if s.contains(&0) {
                    self.ep_puts(b"<");
                    for &b in s {
                        self.ep_puts(format!("{b:02x}").as_bytes());
                    }
                    self.ep_puts(b">");
                } else {
                    self.ep_puts(b"(");
                    for &b in s {
                        if b == b'(' || b == b')' || b == b'\\' {
                            self.ep_puts(&[b'\\', b]);
                        } else if !(0x20..=0x7f).contains(&b) {
                            self.ep_puts(format!("\\{b:03o}").as_bytes());
                        } else {
                            self.pdf_out(b);
                        }
                    }
                    self.ep_puts(b")");
                }
            }
            Obj::Name(n) => self.ep_name(n),
            Obj::Null => self.ep_puts(b"null"),
            Obj::Array(a) => {
                self.ep_puts(b"[");
                for x in a {
                    if !matches!(x, Obj::Name(_)) {
                        self.ep_puts(b" ");
                    }
                    self.ep_object(c, x)?;
                }
                self.ep_puts(b"]");
            }
            Obj::Dict(d) => {
                self.ep_puts(b"<<\n");
                self.ep_dict(c, d)?;
                self.ep_puts(b">>");
            }
            Obj::Stream(s) => {
                self.ep_puts(b"<<\n");
                self.ep_dict(c, &s.dict)?;
                self.ep_puts(b">>\n");
                self.ep_puts(b"stream\n");
                let raw = c.doc.raw(s).to_vec();
                self.ep_stream_bytes(&raw);
                self.ep_puts(b"\nendstream");
            }
            Obj::Ref(r) => {
                if r.num == 0 {
                    return Err(fail(
                        &c.name,
                        "PDF inclusion: reference to invalid object (is the included pdf broken?)",
                    ));
                }
                let n = self.ep_add(c, *r)?;
                self.ep_puts(format!("{n} 0 R").as_bytes());
            }
            Obj::Cmd(_) | Obj::Error | Obj::Eof => {
                return Err(fail(
                    &c.name,
                    &format!("PDF inclusion: type <{}> cannot be copied", o.type_name()),
                ));
            }
        }
        Ok(())
    }

    /// `copyFontResources`.
    fn ep_fonts(&mut self, c: &mut Copy<'_>, o: &Obj) -> Result<(), PdfError> {
        let Obj::Dict(d) = o else {
            return Err(fail(
                &c.name,
                &format!(
                    "PDF inclusion: invalid font resources dict type <{}>",
                    o.type_name()
                ),
            ));
        };
        self.ep_puts(b"/Font << ");
        for (k, v) in &d.0 {
            match v {
                Obj::Ref(r) => {
                    if !c.objs.iter().any(|o| o.r == *r)
                        && let Some(rep) = self.replacement(c.doc, *r, c.src)?
                    {
                        // copyFont, replacing the font
                        let (key, fd_objnum) = self.epdf_fd(&rep);
                        // addFontDesc
                        if !c.objs.iter().any(|o| o.r == rep.desc_ref) {
                            c.objs.push(InObj {
                                r: rep.desc_ref,
                                objnum: fd_objnum,
                                written: false,
                                kind: InKind::FontDesc,
                            });
                        }
                        self.ep_name(k);
                        // addEncoding, then addFont
                        let enc = self.new_objnum_();
                        c.encs.push((enc, rep.names));
                        let objnum = self.new_objnum_();
                        c.objs.push(InObj {
                            r: *r,
                            objnum,
                            written: false,
                            kind: InKind::Font(key, enc),
                        });
                        self.ep_puts(format!(" {objnum} 0 R ").as_bytes());
                        continue;
                    }
                    // copyFont: an object already copied is followed by a
                    // space, a new one is not
                    let seen = c.objs.iter().any(|o| o.r == *r);
                    self.ep_name(k);
                    self.ep_puts(b" ");
                    self.ep_object(c, v)?;
                    if seen {
                        self.ep_puts(b" ");
                    }
                }
                Obj::Dict(_) => {
                    self.ep_name(k);
                    self.ep_puts(b" ");
                    self.ep_object(c, v)?;
                }
                _ => {
                    return Err(fail(
                        &c.name,
                        &format!(
                            "PDF inclusion: invalid font in reference type <{}>",
                            v.type_name()
                        ),
                    ));
                }
            }
        }
        self.ep_puts(b">>\n");
        Ok(())
    }

    /// `copyProcSet`.
    fn ep_procset(&mut self, c: &Copy<'_>, o: &Obj) -> Result<(), PdfError> {
        let Obj::Array(a) = o else {
            return Err(fail(
                &c.name,
                &format!(
                    "PDF inclusion: invalid ProcSet array type <{}>",
                    o.type_name()
                ),
            ));
        };
        self.ep_puts(b"/ProcSet [ ");
        for x in a {
            let Obj::Name(n) = x else {
                return Err(fail(
                    &c.name,
                    &format!(
                        "PDF inclusion: invalid ProcSet entry type <{}>",
                        x.type_name()
                    ),
                ));
            };
            self.ep_name(n);
            self.ep_puts(b" ");
        }
        self.ep_puts(b"]\n");
        Ok(())
    }

    /// `copyOtherResources`.
    fn ep_other(&mut self, c: &mut Copy<'_>, o: &Obj, key: &[u8]) -> Result<(), PdfError> {
        let ok = if key == b"Subtype" {
            matches!(o, Obj::Name(_))
        } else {
            matches!(o, Obj::Dict(_))
        };
        if !ok {
            // a warning; the entry is left out
            return Ok(());
        }
        self.ep_name(key);
        self.ep_puts(b" ");
        self.ep_object(c, o)
    }

    /// `writeRefs`.
    fn ep_write_refs(&mut self, c: &mut Copy<'_>) -> Result<(), PdfError> {
        let mut i = 0;
        while i < c.objs.len() {
            if !c.objs[i].written {
                c.objs[i].written = true;
                let o = c.doc.fetch(c.objs[i].r);
                match c.objs[i].kind.clone() {
                    InKind::FontDesc => {}
                    InKind::Font(key, enc) => {
                        // copyFontDict
                        self.begin_obj(c.objs[i].objnum, 2);
                        let Obj::Dict(d) = &o else {
                            return Err(fail(&c.name, "PDF inclusion: invalid dict type"));
                        };
                        self.ep_puts(b"<<\n");
                        for (k, v) in &d.0 {
                            if k.starts_with(b"FontDescriptor")
                                || k.starts_with(b"BaseFont")
                                || k.starts_with(b"Encoding")
                            {
                                continue;
                            }
                            self.ep_name(k);
                            self.ep_puts(b" ");
                            self.ep_object(c, v)?;
                            self.ep_puts(b"\n");
                        }
                        let fd = self.epdf_fds.get(&key).map_or(0, |e| e.objnum);
                        let fname = self.epdf_fn_objnum(&key);
                        self.ep_puts(format!("/FontDescriptor {fd} 0 R\n").as_bytes());
                        self.ep_puts(format!("/BaseFont {fname} 0 R\n").as_bytes());
                        self.ep_puts(format!("/Encoding {enc} 0 R\n").as_bytes());
                        self.ep_puts(b">>");
                        self.ep_puts(b"\n");
                        self.end_obj();
                    }
                    InKind::Other => {
                        let level = if matches!(o, Obj::Stream(_)) { 0 } else { 2 };
                        self.begin_obj(c.objs[i].objnum, level);
                        self.ep_object(c, &o)?;
                        self.ep_puts(b"\n");
                        self.end_obj();
                    }
                }
            }
            i += 1;
        }
        Ok(())
    }

    /// `write_epdf`: the image's dictionary begun.
    #[allow(clippy::too_many_lines)]
    fn write_epdf(
        &mut self,
        image: &Image,
        im: &PdfImage,
        src: &dyn FontSource,
    ) -> Result<(), PdfError> {
        let file = image.name.clone();
        let sep: &[u8] = if self.p.ptex_use_underscore > 0 {
            b"_"
        } else {
            b"."
        };
        let suppress = self.p.suppress_ptex_info;
        let objs = {
            let d = &mut self.epdf_docs[im.doc];
            d.occurrences -= 1;
            std::mem::take(&mut d.objs)
        };
        let doc =
            Doc::open(&image.data).map_err(|_| fail(&file, "xpdf: reading PDF image failed"))?;
        let page = doc
            .page(usize::try_from(im.page).unwrap_or(0))
            .ok_or_else(|| fail(&file, "xpdf: reading PDF image failed"))?;
        let mut c = Copy {
            doc: &doc,
            objs,
            name: file.clone(),
            encs: Vec::new(),
            src,
        };
        let page_obj = doc.fetch(page.dict_ref);
        let empty = Dict::default();
        let page_dict = page_obj.as_dict().unwrap_or(&empty);
        let rotate = page.rotate;
        self.ep_puts(b"/Type /XObject\n");
        self.ep_puts(b"/Subtype /Form\n");
        self.ep_puts(b"/FormType 1\n");
        if suppress & 0x02 == 0 {
            let mut l = b"/PTEX".to_vec();
            l.extend_from_slice(sep);
            l.extend_from_slice(b"FileName (");
            l.extend_from_slice(&pdf_string(&file));
            l.extend_from_slice(b")\n");
            self.ep_puts(&l);
        }
        if suppress & 0x04 == 0 {
            let mut l = b"/PTEX".to_vec();
            l.extend_from_slice(sep);
            l.extend_from_slice(format!("PageNumber {}\n", im.page).as_bytes());
            self.ep_puts(&l);
        }
        if suppress & 0x08 == 0
            && let Some(Obj::Ref(info)) = doc.trailer.get(b"Info")
        {
            let mut l = b"/PTEX".to_vec();
            l.extend_from_slice(sep);
            l.extend_from_slice(b"InfoDict ");
            self.ep_puts(&l);
            let n = self.ep_add(&mut c, *info)?;
            self.ep_puts(format!("{n} 0 R").as_bytes());
            self.ep_puts(b"\n");
        }
        let b = page_box(&page, im.page_box);
        if rotate != 0 && rotate % 90 == 0 {
            let m = match rotate {
                90 => Some([0.0, -1.0, 1.0, 0.0, b[0] - b[1], b[1] + b[2]]),
                180 => Some([-1.0, 0.0, 0.0, -1.0, b[0] + b[2], b[1] + b[3]]),
                270 => Some([0.0, 1.0, -1.0, 0.0, b[0] + b[3], b[1] - b[0]]),
                _ => None,
            };
            if let Some(m) = m {
                let mut l = b"/Matrix [".to_vec();
                l.extend_from_slice(&eight(&m));
                l.extend_from_slice(b"]\n");
                self.ep_puts(&stripzeros(&l));
            }
        }
        let mut l = b"/BBox [".to_vec();
        l.extend_from_slice(&eight(&b));
        l.extend_from_slice(b"]\n");
        self.ep_puts(&stripzeros(&l));
        for key in [
            &b"LastModified"[..],
            b"Metadata",
            b"PieceInfo",
            b"SeparationInfo",
        ] {
            if let Some(v) = page_dict.get(key)
                && *v != Obj::Null
            {
                self.ep_newline();
                self.ep_puts(b"/");
                self.ep_puts(key);
                self.ep_puts(b" ");
                self.ep_object(&mut c, v)?;
            }
        }
        let mut sep_group = None;
        if let Some(g) = page_dict.get(b"Group")
            && *g != Obj::Null
        {
            let val = self.page_group_val;
            if val == 0 {
                self.ep_newline();
                self.ep_puts(b"/Group ");
                self.ep_object(&mut c, g)?;
            } else {
                let Obj::Dict(gd) = doc.follow(g) else {
                    return Err(fail(&file, "PDF inclusion: /Group dict missing"));
                };
                sep_group = Some((val, gd));
                self.ep_puts(format!("/Group {val} 0 R").as_bytes());
                self.ep_puts(b"\n");
            }
        }
        if let Some(res) = &page.resources {
            self.ep_newline();
            self.ep_puts(b"/Resources <<\n");
            for (k, v) in &res.0 {
                let v = doc.follow(v);
                match &k[..] {
                    b"Font" => self.ep_fonts(&mut c, &v)?,
                    b"ProcSet" => self.ep_procset(&c, &v)?,
                    _ => self.ep_other(&mut c, &v, k)?,
                }
            }
            self.ep_puts(b">>\n");
        }
        match doc.lookup(page_dict, b"Contents") {
            Obj::Stream(s) => self.ep_contents(&doc, &mut c, &s)?,
            Obj::Array(a) => {
                self.begin_stream();
                self.last_byte = 0;
                for (i, x) in a.iter().enumerate() {
                    if let Obj::Stream(s) = doc.follow(x) {
                        let decoded = doc.decode(&s);
                        self.ep_stream_bytes(&decoded);
                    }
                    if i + 1 < a.len() {
                        self.ep_newline();
                    }
                }
                self.end_stream();
            }
            _ => {
                self.begin_stream();
                self.end_stream();
            }
        }
        // writeEncodings: `encodingList` is newest first
        for (objnum, names) in std::mem::take(&mut c.encs).into_iter().rev() {
            self.epdf_write_enc(&names, objnum);
        }
        if let Some((val, gd)) = sep_group {
            self.begin_obj(val, 2);
            self.ep_object(&mut c, &Obj::Dict(gd))?;
            self.ep_puts(b"\n");
            self.end_obj();
            self.page_group_val = 0;
        }
        self.ep_write_refs(&mut c)?;
        self.epdf_docs[im.doc].objs = c.objs;
        Ok(())
    }

    /// A page's single content stream copied as it is.
    fn ep_contents(&mut self, doc: &Doc, c: &mut Copy<'_>, s: &Stream) -> Result<(), PdfError> {
        if doc.lookup(&s.dict, b"F") != Obj::Null {
            return Err(fail(&c.name, "PDF inclusion: Unsupported external stream"));
        }
        let len = doc.lookup(&s.dict, b"Length");
        self.ep_puts(b"/Length ");
        self.ep_object(c, &len)?;
        self.ep_puts(b"\n");
        let filter = doc.lookup(&s.dict, b"Filter");
        if filter != Obj::Null {
            self.ep_puts(b"/Filter ");
            self.ep_object(c, &filter)?;
            self.ep_puts(b"\n");
            let parms = doc.lookup(&s.dict, b"DecodeParms");
            if parms != Obj::Null {
                self.ep_puts(b"/DecodeParms ");
                self.ep_object(c, &parms)?;
                self.ep_puts(b"\n");
            }
        }
        self.ep_puts(b">>\nstream\n");
        let raw = doc.raw(s).to_vec();
        self.ep_stream_bytes(&raw);
        self.end_raw_stream();
        Ok(())
    }
}

/// `scale_image` (pdfTeX §1553).
fn scale_image(xi: &mut XImage, image: &Image, default_res: i32) -> Result<(), PdfError> {
    let (x, y) = image.shown();
    let (mut xr, mut yr) = if matches!(image.rotate(), 90 | 270) {
        (image.y_res, image.x_res)
    } else {
        (image.x_res, image.y_res)
    };
    if xr > 65535 || yr > 65535 {
        (xr, yr) = (0, 0);
    }
    if x <= 0 || y <= 0 || xr < 0 || yr < 0 {
        return Err(pdf_error("ext1", "invalid image dimensions"));
    }
    if (xr != 0 || yr != 0) && (x / ONE_INCH >= xr || y / ONE_INCH >= yr) {
        (xr, yr) = (0, 0);
    }
    let pdf = matches!(image.kind, Kind::Pdf(_));
    if !pdf {
        let default_res = default_res.clamp(0, 65535);
        if default_res > 0 && (xr == 0 || yr == 0) {
            (xr, yr) = (default_res, default_res);
        }
    }
    let running = |v: Scaled| v == RUNNING;
    let (mut w, mut h) = (0, 0);
    if pdf {
        (w, h) = (x, y);
    } else if running(xi.width) && running(xi.height) {
        if xr > 0 && yr > 0 {
            w = ext_xn_over_d(ONE_HUNDRED_INCH, i64::from(x), 100 * i64::from(xr));
            h = ext_xn_over_d(ONE_HUNDRED_INCH, i64::from(y), 100 * i64::from(yr));
        } else {
            w = ext_xn_over_d(ONE_HUNDRED_INCH, i64::from(x), 7200);
            h = ext_xn_over_d(ONE_HUNDRED_INCH, i64::from(y), 7200);
        }
    }
    let (x, y) = (i64::from(x), i64::from(y));
    if running(xi.width) && running(xi.height) && running(xi.depth) {
        (xi.width, xi.height, xi.depth) = (w, h, 0);
    } else if running(xi.width) {
        if running(xi.height) {
            xi.width = ext_xn_over_d(h, x, y);
            xi.height = h - xi.depth;
        } else if running(xi.depth) {
            xi.width = ext_xn_over_d(xi.height, x, y);
            xi.depth = 0;
        } else {
            xi.width = ext_xn_over_d(xi.height + xi.depth, x, y);
        }
    } else if running(xi.height) && running(xi.depth) {
        xi.height = ext_xn_over_d(xi.width, y, x);
        xi.depth = 0;
    } else if running(xi.height) {
        xi.height = ext_xn_over_d(xi.width, y, x) - xi.depth;
    } else if running(xi.depth) {
        xi.depth = 0;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prints_numbers_as_pdftex() {
        assert_eq!(num_to_pdf(1.5), b"1.5");
        assert_eq!(num_to_pdf(-0.25), b"-0.25");
        assert_eq!(num_to_pdf(3.0), b"3");
    }

    #[test]
    fn strips_zeros_as_utils_c() {
        assert_eq!(
            stripzeros(b"[0.00000000 1.50000000 612.00000000]"),
            b"[0 1.5 612]"
        );
    }
}
