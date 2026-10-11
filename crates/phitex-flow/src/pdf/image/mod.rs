//! pdfTeX's images read for `\pdfximage` (writeimg.c, writejpg.c,
//! writepng.c's `read_png_info`, pdftoepdf.cc's `read_pdf_info`, and
//! pdftex.web's `scale_image`): a pure function of the file's bytes and
//! what the command asked for, made where the command is. Writing an
//! image is the fold's (`fold::image`), at the first page showing it.
//! Copied from phitex-core's `pdf/image.rs`, `pdf/writepng.rs` and
//! `pdf/epdf.rs`, with the engine taken out.

use std::sync::Arc;

use super::lib::pdfread::{Doc, Obj};
use super::lib::png;
use super::num::ONE_HUNDRED_INCH;

/// `one_inch`.
const ONE_INCH: i32 = (ONE_HUNDRED_INCH + 50) / 100;

/// image.h's `IMAGE_COLOR_*`: the `/ProcSet` an image needs.
pub const IMAGE_COLOR_B: i32 = 1;
pub const IMAGE_COLOR_C: i32 = 2;
pub const IMAGE_COLOR_I: i32 = 4;

pub const JPG_GRAY: u8 = 1;
pub const JPG_RGB: u8 = 3;
pub const JPG_CMYK: u8 = 4;

/// `pdf_box_spec_*`.
pub const BOX_MEDIA: i32 = 1;
pub const BOX_CROP: i32 = 2;
pub const BOX_BLEED: i32 = 3;
pub const BOX_TRIM: i32 = 4;
pub const BOX_ART: i32 = 5;

/// A page of a PDF file (`pdf_image_struct` and what `read_pdf_info`
/// fills in of the entry).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PdfImage {
    /// The page included, and the box (`pdf_box_spec_*`).
    pub page: i32,
    pub page_box: i32,
    /// `epdf_orig_x`, `epdf_orig_y`.
    pub orig_x: i32,
    pub orig_y: i32,
    /// `img_rotate`.
    pub rotate: i32,
    /// `img_pages`: the document's pages.
    pub pages: i32,
    /// The page has a `/Group`.
    pub group: bool,
}

/// A read image (`image_entry`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Image {
    /// The path as found (`img_name`), printed where it is written.
    pub name: Vec<u8>,
    pub width: i32,
    pub height: i32,
    pub x_res: i32,
    pub y_res: i32,
    /// `img_color`: `IMAGE_COLOR_*` bits.
    pub color: i32,
    pub colorspace_ref: i32,
    pub bits: i32,
    /// A JPEG's colour space (`JPG_*`).
    pub color_space: u8,
    pub data: Arc<[u8]>,
    pub pdf: Option<PdfImage>,
    pub png: bool,
    /// A PNG with an alpha channel in PDF 1.4 or later: it asks for the
    /// job's transparency group (`img_group_ref`, numbered by the fold).
    pub png_group: bool,
}

impl Image {
    /// `img_rotate`.
    pub fn rotate(&self) -> i32 {
        self.pdf.as_ref().map_or(0, |p| p.rotate)
    }

    /// `img_pages`.
    pub fn pages(&self) -> i32 {
        self.pdf.as_ref().map_or(1, |p| p.pages)
    }

    /// `img_width`, `img_height` as the page shows them (a page turned a
    /// quarter has them swapped).
    pub fn shown(&self) -> (i32, i32) {
        if matches!(self.rotate(), 90 | 270) { (self.height, self.width) } else { (self.width, self.height) }
    }

    /// `\pdfximagebbox`'s value `j` (1 to 4): a PDF page's box, else 0.
    pub fn bbox(&self, j: i32) -> i32 {
        self.pdf.as_ref().map_or(0, |p| match j {
            1 => p.orig_x,
            2 => p.orig_y,
            3 => p.orig_x + self.width,
            _ => p.orig_y + self.height,
        })
    }
}

/// web2c's `ext_xn_over_d`: `x*n/d` in doubles, rounded (halves away
/// from zero), truncated to an integer.
pub fn ext_xn_over_d(x: i32, n: i32, d: i32) -> i32 {
    let mut r = f64::from(x) * f64::from(n) / f64::from(d);
    if r > f64::EPSILON {
        r += 0.5;
    } else {
        r -= 0.5;
    }
    r as i32
}

/// writejpg.c's `read_jpg_info`, or `pdftex_fail`'s message.
pub fn read_jpg_info(name: Vec<u8>, data: Arc<[u8]>, colorspace_ref: i32, pdf_12: bool) -> Result<Image, String> {
    let d = &data[..];
    let two = |i: usize| -> u16 { u16::from(*d.get(i).unwrap_or(&0)) << 8 | u16::from(*d.get(i + 1).unwrap_or(&0)) };
    if two(0) != 0xFFD8 {
        return Err("reading JPEG image failed (no JPEG header found)".into());
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
                    // (`img_xres(img) *= 2.54`, an integer)
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
        // (Exif: the resolution its TIFF fields give, else none)
        0xFFE1 => {
            let length = usize::from(two(4)).wrapping_sub(2) & 0xFFFF;
            if length > 5 {
                // (`fread` of the signature short: pdfTeX returns, the image
                // with no size)
                if d.len() < 11 {
                    return Err("invalid image dimensions".into());
                }
                if &d[6..11] == b"Exif\0" {
                    (x_res, y_res) = read_app1_exif(d.get(11..).unwrap_or(&[]), length - 5);
                }
            }
        }
        _ => {}
    }
    let mut i = 0;
    loop {
        if i >= d.len() {
            return Err("reading JPEG image failed (premature file end)".into());
        }
        if d[i] != 0xFF {
            return Err("reading JPEG image failed (no marker found)".into());
        }
        let m = d.get(i + 1).copied().unwrap_or(0);
        i += 2;
        match m {
            0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => return Err("unsupported type of compression".into()),
            0xC0..=0xC3 => {
                if m == 0xC2 && pdf_12 {
                    return Err("cannot use progressive DCT with PDF-1.2".into());
                }
                let bits = i32::from(d.get(i + 2).copied().unwrap_or(0));
                let height = i32::from(two(i + 3));
                let width = i32::from(two(i + 5));
                let color_space = d.get(i + 7).copied().unwrap_or(0);
                let color = match color_space {
                    JPG_GRAY => IMAGE_COLOR_B,
                    JPG_RGB | JPG_CMYK => IMAGE_COLOR_C,
                    _ => return Err("Unsupported color space".into()),
                };
                return Ok(Image {
                    name,
                    width,
                    height,
                    x_res,
                    y_res,
                    color,
                    colorspace_ref,
                    bits,
                    color_space,
                    data,
                    pdf: None,
                    png: false,
                    png_group: false,
                });
            }
            // (markers without parameters)
            0xD8 | 0xD9 | 0x01 | 0xD0..=0xD7 => {}
            _ => i += usize::from(two(i)),
        }
    }
}

/// writejpg.c's `read_exif_bytes`: `n` (2 or 4) bytes at `*p`, big- or
/// little-endian, `*p` moved past them (bytes past the segment read as 0).
fn read_exif_bytes(buf: &[u8], p: &mut usize, n: usize, big: bool) -> u32 {
    let at = |i: usize| u32::from(buf.get(i).copied().unwrap_or(0));
    let mut v = 0u32;
    for j in 0..n {
        let i = if big { *p + j } else { *p + n - 1 - j };
        v = (v << 8) | at(i);
    }
    *p += n;
    v
}

/// writejpg.c's `read_APP1_Exif`: the resolution an Exif segment's first
/// IFD gives (`XResolution`, `YResolution`, `ResolutionUnit`; 72 dpi where
/// one is missing), `(0, 0)` if it is not TIFF data. `seg` is what follows
/// the signature, `length` the bytes pdfTeX reads of it. As in C: a
/// rational is `num / den` in integers, and `num`, `den` and `value` are
/// those of the last field that set them.
fn read_app1_exif(seg: &[u8], length: usize) -> (i32, i32) {
    let buf = &seg[..length.min(seg.len())];
    let byte = |i: usize| buf.get(i).copied().unwrap_or(0);
    let mut p = 0usize;
    while p < length && byte(p) == 0 {
        p += 1;
    }
    let tiff = p;
    let big = match (byte(p), byte(p + 1)) {
        (b'M', b'M') => true,
        (b'I', b'I') => false,
        _ => return (0, 0),
    };
    p += 2;
    if read_exif_bytes(buf, &mut p, 2, big) != 42 {
        return (0, 0);
    }
    let i = read_exif_bytes(buf, &mut p, 4, big) as usize;
    p = tiff + i;
    let mut num_fields = read_exif_bytes(buf, &mut p, 2, big) as i32;
    let (mut value, mut num, mut den) = (0i32, 0i32, 0i32);
    let (mut xres, mut yres, mut res_unit) = (72.0f64, 72.0f64, 1.0f64);
    while num_fields > 0 {
        num_fields -= 1;
        let tag = read_exif_bytes(buf, &mut p, 2, big);
        let ty = read_exif_bytes(buf, &mut p, 2, big);
        read_exif_bytes(buf, &mut p, 4, big);
        match ty {
            1 | 7 => {
                value = i32::from(byte(p));
                p += 4;
            }
            3 => {
                value = read_exif_bytes(buf, &mut p, 2, big) as i32;
                p += 2;
            }
            4 | 9 => value = read_exif_bytes(buf, &mut p, 4, big) as i32,
            5 | 10 => {
                value = read_exif_bytes(buf, &mut p, 4, big) as i32;
                let mut rp = tiff.wrapping_add(value as u32 as usize);
                num = read_exif_bytes(buf, &mut rp, 4, big) as i32;
                den = read_exif_bytes(buf, &mut rp, 4, big) as i32;
            }
            _ => p += 4,
        }
        match tag {
            282 if den != 0 => xres = f64::from(num.wrapping_div(den)),
            283 if den != 0 => yres = f64::from(num.wrapping_div(den)),
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

/// png.h's colour types.
pub const PNG_GRAY: u8 = 0;
pub const PNG_RGB: u8 = 2;
pub const PNG_PALETTE: u8 = 3;
pub const PNG_GRAY_ALPHA: u8 = 4;
pub const PNG_RGB_ALPHA: u8 = 6;
/// `PNG_INFO_pHYs`.
const INFO_PHYS: u32 = 0x0080;

/// writepng.c's `read_png_info`.
pub fn read_png_info(name: Vec<u8>, data: Arc<[u8]>, colorspace_ref: i32, major: i32, minor: i32) -> Result<Image, String> {
    let Ok(info) = png::read_info(&data) else {
        return Err("libpng: internal error".into());
    };
    let (mut x_res, mut y_res) = (0, 0);
    if info.get_valid(INFO_PHYS) != 0 {
        // (`png_get_x_pixels_per_meter`: 0 in other units)
        let (x, y, unit) = info.phys.unwrap_or((0, 0, 0));
        let per_meter = |v: u32| if unit == 1 { f64::from(v) } else { 0.0 };
        x_res = (0.0254 * per_meter(x)).round() as i32;
        y_res = (0.0254 * per_meter(y)).round() as i32;
    }
    let color = match info.color_type {
        PNG_PALETTE => IMAGE_COLOR_C | IMAGE_COLOR_I,
        PNG_GRAY | PNG_GRAY_ALPHA => IMAGE_COLOR_B,
        PNG_RGB | PNG_RGB_ALPHA => IMAGE_COLOR_C,
        t => return Err(format!("unsupported type of color_type <{t}>")),
    };
    let png_group = (major > 1 || minor >= 4) && matches!(info.color_type, PNG_GRAY_ALPHA | PNG_RGB_ALPHA);
    Ok(Image {
        name,
        width: i32::try_from(info.width).unwrap_or(i32::MAX),
        height: i32::try_from(info.height).unwrap_or(i32::MAX),
        x_res,
        y_res,
        color,
        colorspace_ref,
        bits: i32::from(info.bit_depth),
        color_space: 0,
        data,
        pdf: None,
        png: true,
        png_group,
    })
}

/// writeimg.c's `bp2int`: a `float` of big points in scaled points,
/// rounded (halves away from zero).
pub fn bp2int(p: f32) -> i32 {
    let x = f64::from(p) * (f64::from(super::num::ONE_HUNDRED_BP) / 100.0);
    let t = x as i64;
    let f = x - t as f64;
    let r = if f >= 0.5 {
        t + 1
    } else if f <= -0.5 {
        t - 1
    } else {
        t
    };
    i32::try_from(r).unwrap_or(if r < 0 { i32::MIN } else { i32::MAX })
}

/// The rectangle of page box `spec`.
pub fn page_box(p: &super::lib::pdfread::Page, spec: i32) -> Option<[f64; 4]> {
    Some(match spec {
        BOX_MEDIA => p.media,
        BOX_CROP => p.crop,
        BOX_BLEED => p.bleed,
        BOX_TRIM => p.trim,
        BOX_ART => p.art,
        _ => return None,
    })
}

/// pdftoepdf.cc's `read_pdf_info`: the page (by `named` destination if
/// given, else number `page_num`) and its box `spec`; the version allowed
/// (`major.minor`, and what a newer one is: `errorlevel`). The image and
/// a warning, or `pdftex_fail`'s message.
#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
pub fn read_pdf_info(
    name: Vec<u8>,
    data: Arc<[u8]>,
    named: Option<&[u8]>,
    page_num: i32,
    spec: i32,
    major: i32,
    minor: i32,
    errorlevel: i32,
    colorspace_ref: i32,
) -> Result<(Image, Option<String>), String> {
    let Ok(doc) = Doc::open(&data) else {
        return Err("xpdf: reading PDF image failed".into());
    };
    let mut warning = None;
    let found = doc.version as f32;
    let wanted = (f64::from(major) + f64::from(minor) * 0.1) as f32;
    if f64::from(found) > f64::from(wanted) + 0.01 {
        let msg = format!(
            "PDF inclusion: found PDF version <{:.1}>, but at most version <{:.1}> allowed",
            f64::from(found),
            f64::from(wanted)
        );
        if errorlevel > 0 {
            return Err(msg);
        } else if errorlevel == 0 {
            warning = Some(msg);
        }
    }
    let pages = i32::try_from(doc.num_pages()).unwrap_or(i32::MAX);
    let page_num = if let Some(dest) = named {
        let Some(n) = doc.find_dest(dest) else {
            return Err(format!("PDF inclusion: invalid destination <{}>", String::from_utf8_lossy(dest)));
        };
        if n == 0 {
            return Err(format!("PDF inclusion: destination is not a page <{}>", String::from_utf8_lossy(dest)));
        }
        i32::try_from(n).unwrap_or(i32::MAX)
    } else {
        if page_num <= 0 || page_num > pages {
            return Err(format!("PDF inclusion: required page does not exist <{pages}>"));
        }
        page_num
    };
    let Some(page) = doc.page(usize::try_from(page_num).unwrap_or(0)) else {
        return Err("xpdf: reading PDF image failed".into());
    };
    let Some(b) = page_box(&page, spec) else {
        return Err(format!("PDF inclusion: unknown value of pagebox spec ({spec})"));
    };
    // (the `float` globals `epdf_orig_x`, `epdf_width`, …)
    let f = |x: f64| x as f32;
    let (orig_x, width) = if b[2] > b[0] { (f(b[0]), f(b[2] - b[0])) } else { (f(b[2]), f(b[0] - b[2])) };
    let (orig_y, height) = if b[3] > b[1] { (f(b[1]), f(b[3] - b[1])) } else { (f(b[3]), f(b[1] - b[3])) };
    let group = matches!(doc.fetch(page.dict_ref).as_dict().map(|d| doc.lookup(d, b"Group")), Some(Obj::Dict(_)));
    let image = Image {
        name,
        width: bp2int(width),
        height: bp2int(height),
        x_res: 0,
        y_res: 0,
        color: 0,
        colorspace_ref,
        bits: 0,
        color_space: 0,
        data: data.clone(),
        pdf: Some(PdfImage {
            page: page_num,
            page_box: spec,
            orig_x: bp2int(orig_x),
            orig_y: bp2int(orig_y),
            rotate: page.rotate,
            pages,
            group,
        }),
        png: false,
        png_group: false,
    };
    Ok((image, warning))
}

/// writeimg.c's `read_image` of file `name` (as found) with bytes `data`:
/// its type by its first bytes (`checktypebyheader`) or its name
/// (`checktypebyextension`), read. The image and a warning, or
/// `pdftex_fail`'s message.
#[allow(clippy::too_many_arguments)]
pub fn read_image(
    name: &str,
    data: Arc<[u8]>,
    page: i32,
    named: Option<&[u8]>,
    colorspace: i32,
    pagebox: i32,
    major: i32,
    minor: i32,
    errorlevel: i32,
) -> Result<(Image, Option<String>), String> {
    let d = &data[..];
    if d.len() < 8 {
        return Err("reading image file failed".into());
    }
    let suffix = name.rfind('.').map(|i| name[i..].to_ascii_lowercase());
    let kind = if d.starts_with(b"\xFF\xD8") {
        "jpg"
    } else if d.starts_with(b"\x89PNG\r\n\x1A\n") {
        "png"
    } else if d.starts_with(b"\x97JB2\r\n\x1A\n") {
        "jb2"
    } else if d.starts_with(b"%PDF-1.") {
        "pdf"
    } else {
        match suffix.as_deref() {
            Some(".png") => "png",
            Some(".jpg" | ".jpeg") => "jpg",
            Some(".jbig2" | ".jb2") => "jb2",
            Some(".pdf") => "pdf",
            _ => return Err("unknown type of image".into()),
        }
    };
    let n = name.as_bytes().to_vec();
    match kind {
        "pdf" => read_pdf_info(n, data, named, page, pagebox, major, minor, errorlevel, colorspace),
        "jpg" => read_jpg_info(n, data, colorspace, major == 1 && minor <= 2).map(|i| (i, None)),
        "png" => read_png_info(n, data, colorspace, major, minor).map(|i| (i, None)),
        _ => Err("JBIG2 images are not supported yet".into()),
    }
}

/// pdftex.web's `scale_image`: the image's width, height and depth from
/// those given (`item::RUNNING` where not), and the warnings it gives;
/// `Err`: `pdf_error`'s message.
/// An image's size as scaled, and the warnings scaling gave.
pub type Scaled = ((i32, i32, i32), Vec<String>);

pub fn scale(image: &Image, given: (i32, i32, i32), image_resolution: i32) -> Result<Scaled, String> {
    use crate::item::RUNNING;
    let mut warnings = Vec::new();
    let (x, y) = image.shown();
    let (mut xr, mut yr) = if matches!(image.rotate(), 90 | 270) { (image.y_res, image.x_res) } else { (image.x_res, image.y_res) };
    if xr > 65535 || yr > 65535 {
        (xr, yr) = (0, 0);
        warnings.push("pdfTeX warning (ext1): too large image resolution ignored".into());
    }
    if x <= 0 || y <= 0 || xr < 0 || yr < 0 {
        return Err("invalid image dimensions".into());
    }
    if (xr != 0 || yr != 0) && (x / ONE_INCH >= xr || y / ONE_INCH >= yr) {
        (xr, yr) = (0, 0);
        warnings.push("pdfTeX warning (ext1): too small image resolution ignored".into());
    }
    let (mut wd, mut ht, mut dp) = given;
    let running = |v: i32| v == RUNNING;
    let (mut w, mut h) = (0, 0);
    if image.pdf.is_some() {
        (w, h) = (x, y);
    } else {
        let default_res = image_resolution.clamp(0, 65535);
        if default_res > 0 && (xr == 0 || yr == 0) {
            (xr, yr) = (default_res, default_res);
        }
        if running(wd) && running(ht) {
            if xr > 0 && yr > 0 {
                w = ext_xn_over_d(ONE_HUNDRED_INCH, x, 100 * xr);
                h = ext_xn_over_d(ONE_HUNDRED_INCH, y, 100 * yr);
            } else {
                w = ext_xn_over_d(ONE_HUNDRED_INCH, x, 7200);
                h = ext_xn_over_d(ONE_HUNDRED_INCH, y, 7200);
            }
        }
    }
    if running(wd) && running(ht) && running(dp) {
        (wd, ht, dp) = (w, h, 0);
    } else if running(wd) {
        if running(ht) {
            wd = ext_xn_over_d(h, x, y);
            ht = h - dp;
        } else if running(dp) {
            wd = ext_xn_over_d(ht, x, y);
            dp = 0;
        } else {
            wd = ext_xn_over_d(ht + dp, x, y);
        }
    } else if running(ht) && running(dp) {
        ht = ext_xn_over_d(wd, y, x);
        dp = 0;
    } else if running(ht) {
        ht = ext_xn_over_d(wd, y, x) - dp;
    } else if running(dp) {
        dp = 0;
    }
    Ok(((wd, ht, dp), warnings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_jfif_header() {
        let mut d = b"\xFF\xD8\xFF\xE0\x00\x10JFIF\x00\x01\x01\x01\x00\x48\x00\x48\x00\x00".to_vec();
        d.extend_from_slice(b"\xFF\xC0\x00\x11\x08\x00\x20\x00\x40\x03");
        let i = read_jpg_info(Vec::new(), Arc::from(d), 0, false).unwrap();
        assert_eq!((i.width, i.height, i.bits, i.x_res, i.color), (64, 32, 8, 72, IMAGE_COLOR_C));
        assert_eq!(ext_xn_over_d(7, 3, 2), 11);
        assert_eq!(ext_xn_over_d(-7, 3, 2), -11);
        assert_eq!(bp2int(612.0), 40_258_437);
        assert_eq!(bp2int(-10.0), -657_818);
    }
}
