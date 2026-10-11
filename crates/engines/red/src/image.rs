//! E6: images: PNG, JPEG, PDF page inclusion (`\pdfximage`), as pdfTeX's
//! `writeimg.c`, `writepng.c`, `writejpg.c` and `epdf.cc` write them.
//!
//! [`info`] / [`info_with`] read what `\pdfximage` needs at scan time (kind,
//! pixel size, resolution, natural size in sp, pages); [`include`] writes the
//! image's objects through an [`ObjSink`] (E1's `pdf::Writer`) when the image
//! is first referenced, and returns its object number. Both are pure functions
//! of the bytes and the parameters passed: nothing here reads the environment
//! except [`Params::read`].
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;

use phitex_codecs::deflate;
use phitex_codecs::pdfread::{self, Dict, Obj, Ref};
use phitex_codecs::png;

use crate::ctx::Ctx;
use crate::prim::ip;
use crate::val::Key;

/// Where objects go: E1's writer. `alloc` reserves an object number;
/// `write_obj` writes the body of object `num` (what goes between
/// `num 0 obj` and `endobj`).
pub trait ObjSink {
    fn alloc(&mut self) -> u32;
    fn write_obj(&mut self, num: u32, body: &[u8]);
}

impl ObjSink for crate::pdf::Writer {
    fn alloc(&mut self) -> u32 {
        crate::pdf::Writer::alloc(self)
    }
    fn write_obj(&mut self, num: u32, body: &[u8]) {
        crate::pdf::Writer::write_obj(self, num, body);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Png,
    Jpeg,
    Pdf,
    Jbig2,
    Unknown,
}

/// The pdfTeX parameters images depend on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Params {
    pub pdf_minor: i32,
    pub compress_level: i32,
    pub image_resolution: i32,
    pub page_box: i32,
    pub inclusion_errorlevel: i32,
    pub inclusion_copy_fonts: i32,
    pub gamma: i32,
    pub image_gamma: i32,
    pub image_hicolor: i32,
    pub image_apply_gamma: i32,
    pub suppress_ptex_info: i32,
    pub decimal_digits: i32,
}

impl Default for Params {
    fn default() -> Params {
        Params {
            pdf_minor: 4,
            compress_level: 9,
            image_resolution: 0,
            page_box: 0,
            inclusion_errorlevel: 0,
            inclusion_copy_fonts: 0,
            gamma: 1000,
            image_gamma: 2200,
            image_hicolor: 1,
            image_apply_gamma: 0,
            suppress_ptex_info: 0,
            decimal_digits: 4,
        }
    }
}

impl Params {
    /// The parameters in force (recorded reads).
    pub fn read(c: &mut dyn Ctx) -> Params {
        let mut g = |n: &str| c.int(&Key::IntPar(ip::of(n)));
        let minor = g("pdfminorversion");
        Params {
            pdf_minor: minor,
            compress_level: g("pdfcompresslevel"),
            image_resolution: g("pdfimageresolution"),
            page_box: g("pdfpagebox"),
            inclusion_errorlevel: g("pdfinclusionerrorlevel"),
            inclusion_copy_fonts: g("pdfinclusioncopyfonts"),
            gamma: g("pdfgamma"),
            image_gamma: g("pdfimagegamma"),
            image_hicolor: g("pdfimagehicolor"),
            image_apply_gamma: g("pdfimageapplygamma"),
            suppress_ptex_info: g("pdfsuppressptexinfo"),
            decimal_digits: g("pdfdecimaldigits"),
        }
    }
}

/// What `\pdfximage` asked for beyond the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    /// `page n` (from 1).
    pub page: i32,
    /// `mediabox` 1 … `artbox` 5; 0: `\pdfpagebox`, else the crop box.
    pub page_box: i32,
}

impl Default for Request {
    fn default() -> Request {
        Request { page: 1, page_box: 0 }
    }
}

/// What a `\pdfximage` node knows of its image.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageInfo {
    pub kind: Kind,
    /// Pixels (PNG, JPEG); for PDF the chosen box in bp, rounded.
    pub x_size: i32,
    pub y_size: i32,
    /// Dots per inch from the file (0: none).
    pub x_res: i32,
    pub y_res: i32,
    /// Natural size in sp (resolution applied; PDF: box rotated).
    pub width: i32,
    pub height: i32,
    pub depth: i32,
    /// PDF: the box's lower left corner in sp (placement subtracts it).
    pub orig_x: i32,
    pub orig_y: i32,
    /// PDF: pages in the file, the page chosen, its /Rotate.
    pub pages: i32,
    pub page: i32,
    pub rotate: i32,
    pub bits: u8,
    pub colors: u8,
    /// pdfTeX's error (`! ...`) when the file is not an image it reads.
    pub error: Option<String>,
    /// Warnings to print (`pdfTeX warning: ...`).
    pub warnings: Vec<String>,
}

impl ImageInfo {
    fn empty(kind: Kind) -> ImageInfo {
        ImageInfo {
            kind,
            x_size: 0,
            y_size: 0,
            x_res: 0,
            y_res: 0,
            width: 0,
            height: 0,
            depth: 0,
            orig_x: 0,
            orig_y: 0,
            pages: 1,
            page: 1,
            rotate: 0,
            bits: 0,
            colors: 0,
            error: None,
            warnings: Vec::new(),
        }
    }
    fn err(kind: Kind, msg: impl Into<String>) -> ImageInfo {
        let mut i = ImageInfo::empty(kind);
        i.error = Some(msg.into());
        i
    }
}

const ONE_HUNDRED_INCH: i64 = 7227 * 65536;
const ONE_BP: f64 = 65781.76;

/// pdfTeX's `ext_xn_over_d`: `x * n / d` rounded.
pub fn ext_xn_over_d(x: i64, n: i64, d: i64) -> i32 {
    if d == 0 {
        return 0;
    }
    let r = x as f64 * n as f64 / d as f64;
    let r = if r > 0.0 { r + 0.5 } else { r - 0.5 };
    r.clamp(-1073741823.0, 1073741823.0) as i32
}

/// bp to sp, rounded (`bp2int`).
pub fn bp2sp(bp: f64) -> i32 {
    let r = bp * ONE_BP;
    (if r >= 0.0 { r + 0.5 } else { r - 0.5 }) as i32
}

pub fn kind_of(b: &[u8]) -> Kind {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Kind::Png
    } else if b.starts_with(&[0xFF, 0xD8]) {
        Kind::Jpeg
    } else if b.starts_with(b"\x97JB2\r\n\x1a\n") {
        Kind::Jbig2
    } else if b.len() >= 5 && b.windows(5).take(1024).any(|w| w == b"%PDF-") {
        Kind::Pdf
    } else {
        Kind::Unknown
    }
}

/// The image's info for page 1 and the default box.
pub fn info(bytes: &[u8]) -> ImageInfo {
    info_with(bytes, &Request::default(), &Params::default())
}

pub fn info_with(bytes: &[u8], req: &Request, p: &Params) -> ImageInfo {
    let mut i = match kind_of(bytes) {
        Kind::Png => png_info(bytes),
        Kind::Jpeg => jpeg_info(bytes),
        Kind::Pdf => return pdf_info(bytes, req, p),
        Kind::Jbig2 => {
            return ImageInfo::err(Kind::Jbig2, "JBIG2 images are not supported");
        }
        Kind::Unknown => return ImageInfo::err(Kind::Unknown, "unknown graphics file type"),
    };
    if i.error.is_none() {
        natural_size(&mut i, p);
    }
    i
}

/// `scale_image`'s natural size from pixels and resolution.
fn natural_size(i: &mut ImageInfo, p: &Params) {
    let (mut xr, mut yr) = (i64::from(i.x_res), i64::from(i.y_res));
    if xr <= 0 || yr <= 0 {
        if p.image_resolution > 0 {
            xr = i64::from(p.image_resolution);
            yr = xr;
        } else {
            xr = 72;
            yr = 72;
        }
    }
    i.width = ext_xn_over_d(ONE_HUNDRED_INCH, i64::from(i.x_size), 100 * xr);
    i.height = ext_xn_over_d(ONE_HUNDRED_INCH, i64::from(i.y_size), 100 * yr);
    i.depth = 0;
}

/// `\pdfximage width .. height .. depth ..` against the natural size:
/// `None` is a running dimension. Returns (width, height, depth).
pub fn scale(i: &ImageInfo, w: Option<i32>, h: Option<i32>, d: Option<i32>) -> (i32, i32, i32) {
    let (nw, nh) = (i64::from(i.width), i64::from(i.height));
    match (w, h, d) {
        (None, None, None) => (i.width, i.height, 0),
        (None, h, d) => {
            let (h, d) = (h.unwrap_or(i.height), d.unwrap_or(0));
            (ext_xn_over_d(i64::from(h) + i64::from(d), nw, nh), h, d)
        }
        (Some(w), None, None) => (w, ext_xn_over_d(i64::from(w), nh, nw), 0),
        (Some(w), None, Some(d)) => (w, ext_xn_over_d(i64::from(w), nh, nw) - d, d),
        (Some(w), Some(h), d) => (w, h, d.unwrap_or(0)),
    }
}

// ---------------------------------------------------------------- PNG

fn png_info(b: &[u8]) -> ImageInfo {
    let info = match png::read_info(b) {
        Ok(x) => x,
        Err(e) => return ImageInfo::err(Kind::Png, format!("reading PNG image failed ({e})")),
    };
    let mut i = ImageInfo::empty(Kind::Png);
    i.x_size = info.width as i32;
    i.y_size = info.height as i32;
    i.bits = info.bit_depth;
    i.colors = match info.color_type {
        png::COLOR_RGB | png::COLOR_RGB_ALPHA | png::COLOR_PALETTE => 3,
        _ => 1,
    };
    if let Some((x, y, 1)) = info.phys {
        // pixels per meter → dpi (`png_get_x_pixels_per_inch`)
        i.x_res = (f64::from(x) * 0.0254 + 0.5) as i32;
        i.y_res = (f64::from(y) * 0.0254 + 0.5) as i32;
    }
    i
}

/// The concatenated IDAT data.
fn idat(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut at = 8;
    while at + 8 <= b.len() {
        let len = u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]) as usize;
        let ty = &b[at + 4..at + 8];
        let start = at + 8;
        let end = start.saturating_add(len).min(b.len());
        if ty == b"IDAT" {
            out.extend_from_slice(&b[start..end]);
        } else if ty == b"IEND" {
            break;
        }
        at = end + 4;
    }
    out
}

/// libpng's `png_gamma_8bit_correct` / 16-bit analogue with exponent `g`.
fn gamma8(v: u8, g: f64) -> u8 {
    (255.0 * (f64::from(v) / 255.0).powf(g) + 0.5).floor().clamp(0.0, 255.0) as u8
}
fn gamma16(v: u16, g: f64) -> u16 {
    (65535.0 * (f64::from(v) / 65535.0).powf(g) + 0.5).floor().clamp(0.0, 65535.0) as u16
}

/// Splits interleaved samples into the colour bytes and the alpha bytes
/// (`bps` bytes a sample; alpha the last channel).
fn split_alpha(rows: &[u8], channels: usize, bps: usize) -> (Vec<u8>, Vec<u8>) {
    let px = channels * bps;
    let mut c = Vec::with_capacity(rows.len() / channels * (channels - 1));
    let mut a = Vec::with_capacity(rows.len() / channels);
    for p in rows.chunks_exact(px) {
        c.extend_from_slice(&p[..px - bps]);
        a.extend_from_slice(&p[px - bps..]);
    }
    (c, a)
}

fn png_include(w: &mut dyn ObjSink, b: &[u8], p: &Params) -> Result<u32, String> {
    let info = png::read_info(b).map_err(|e| format!("reading PNG image failed ({e})"))?;
    let has_trns = info.get_valid(png::INFO_TRNS) != 0;
    let apply_gamma = p.image_apply_gamma > 0;
    let ct = info.color_type;
    let bd = info.bit_depth;
    let num = w.alloc();
    let (width, height) = (info.width, info.height);

    // copy the IDAT data as it is, with the predictors (`copy_png`)
    let copy = p.pdf_minor > 1
        && info.interlace == 0
        && !has_trns
        && !apply_gamma
        && matches!(ct, png::COLOR_GRAY | png::COLOR_RGB | png::COLOR_PALETTE)
        && (bd <= 8 || (bd == 16 && p.image_hicolor > 0 && p.pdf_minor >= 5));
    if copy {
        let colors = if ct == png::COLOR_RGB { 3 } else { 1 };
        let cs = colorspace(w, &info, ct, p);
        let mut d = head(width, height, bd);
        let _ = write!(d, "/ColorSpace {cs}\n/Filter /FlateDecode\n/DecodeParms << /Colors {colors} /Columns {width} /BitsPerComponent {bd} /Predictor 10 >>\n");
        w.write_obj(num, &stream_obj(&d, &idat(b)));
        return Ok(num);
    }

    let t = png::Transforms {
        trns_to_alpha: has_trns,
        strip_alpha: false,
        strip_16: bd == 16 && (p.image_hicolor == 0 || p.pdf_minor < 5),
    };
    let (oct, _) = png::output(&info, t);
    let t = png::Transforms {
        strip_alpha: p.pdf_minor < 4 && oct & 4 != 0,
        ..t
    };
    let img = png::read_image(b, &info, t).map_err(|e| format!("reading PNG image failed ({e})"))?;
    let mut rows = img.rows;
    let (ict, ibd) = (img.color_type, img.bit_depth);
    let bps = if ibd == 16 { 2 } else { 1 };
    let channels = match ict {
        png::COLOR_GRAY | png::COLOR_PALETTE => 1,
        png::COLOR_GRAY_ALPHA => 2,
        png::COLOR_RGB => 3,
        _ => 4,
    };
    let mut info = info;
    if apply_gamma {
        let screen = f64::from(p.gamma) / 1000.0;
        let file = if info.get_valid(png::INFO_GAMA) != 0 {
            f64::from(info.gamma) / 100_000.0
        } else {
            1000.0 / f64::from(p.image_gamma.max(1))
        };
        let g = 1.0 / (file * screen);
        if ict == png::COLOR_PALETTE {
            for e in &mut info.palette {
                for c in e.iter_mut() {
                    *c = gamma8(*c, g);
                }
            }
        } else if ibd >= 8 {
            let color_ch = if ict & 4 != 0 { channels - 1 } else { channels };
            for px in rows.chunks_exact_mut(channels * bps) {
                for c in 0..color_ch {
                    if bps == 1 {
                        px[c] = gamma8(px[c], g);
                    } else {
                        let v = u16::from_be_bytes([px[2 * c], px[2 * c + 1]]);
                        px[2 * c..2 * c + 2].copy_from_slice(&gamma16(v, g).to_be_bytes());
                    }
                }
            }
        }
    }
    let (color, alpha) = if ict & 4 != 0 {
        let (c, a) = split_alpha(&rows, channels, bps);
        (c, Some(a))
    } else {
        (rows, None)
    };
    let smask = alpha.map(|a| {
        let n = w.alloc();
        let mut d = head(width, height, ibd);
        d.push_str("/ColorSpace /DeviceGray\n");
        let (f, data) = compress(&a, p.compress_level);
        d.push_str(&f);
        w.write_obj(n, &stream_obj(&d, &data));
        n
    });
    let cs = colorspace(w, &info, ict & !4, p);
    let mut d = head(width, height, ibd);
    if let Some(n) = smask {
        let _ = writeln!(d, "/SMask {n} 0 R");
    }
    let _ = writeln!(d, "/ColorSpace {cs}");
    let (f, data) = compress(&color, p.compress_level);
    d.push_str(&f);
    w.write_obj(num, &stream_obj(&d, &data));
    Ok(num)
}

/// The colour space of colour type `ct` (alpha bit off): a palette goes to
/// its own object.
fn colorspace(w: &mut dyn ObjSink, info: &png::Info, ct: u8, p: &Params) -> String {
    match ct {
        png::COLOR_PALETTE => {
            let n = w.alloc();
            let pal: Vec<u8> = info.palette.iter().flatten().copied().collect();
            let (f, data) = compress(&pal, p.compress_level);
            w.write_obj(n, &stream_obj(&f, &data));
            format!("[/Indexed /DeviceRGB {} {n} 0 R]", info.palette.len().max(1) - 1)
        }
        png::COLOR_RGB => "/DeviceRGB".into(),
        _ => "/DeviceGray".into(),
    }
}

fn head(width: u32, height: u32, bits: u8) -> String {
    format!("/Type /XObject\n/Subtype /Image\n/Width {width}\n/Height {height}\n/BitsPerComponent {bits}\n")
}

/// The `/Filter` entry and the data, compressed at `\pdfcompresslevel`.
fn compress(data: &[u8], level: i32) -> (String, Vec<u8>) {
    if level > 0 {
        ("/Filter /FlateDecode\n".into(), deflate::compress(data, level))
    } else {
        (String::new(), data.to_vec())
    }
}

/// A stream object's body: `<< dict /Length n >> stream … endstream`.
fn stream_obj(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut o = Vec::with_capacity(data.len() + dict.len() + 48);
    o.extend_from_slice(b"<<\n");
    o.extend_from_slice(dict.as_bytes());
    o.extend_from_slice(format!("/Length {}\n>>\nstream\n", data.len()).as_bytes());
    o.extend_from_slice(data);
    o.extend_from_slice(b"\nendstream");
    o
}

// ---------------------------------------------------------------- JPEG

struct Jpeg {
    width: u16,
    height: u16,
    bits: u8,
    comps: u8,
    adobe: bool,
    res: Option<(i32, i32)>,
}

fn jpeg_scan(b: &[u8]) -> Result<Jpeg, String> {
    let mut j = Jpeg { width: 0, height: 0, bits: 0, comps: 0, adobe: false, res: None };
    let mut at = 2;
    loop {
        while at < b.len() && b[at] != 0xFF {
            at += 1;
        }
        while at < b.len() && b[at] == 0xFF {
            at += 1;
        }
        if at >= b.len() {
            return Err("reading JPG image failed (no marker found)".into());
        }
        let m = b[at];
        at += 1;
        if m == 0x01 || (0xD0..=0xD7).contains(&m) {
            continue;
        }
        if m == 0xD9 || m == 0xDA {
            return Err("reading JPG image failed (no SOF marker)".into());
        }
        if at + 2 > b.len() {
            return Err("reading JPG image failed (premature end)".into());
        }
        let len = usize::from(u16::from_be_bytes([b[at], b[at + 1]]));
        let seg = &b[at + 2..(at + len).min(b.len())];
        match m {
            0xE0 if seg.starts_with(b"JFIF\0") && seg.len() >= 12 && j.res.is_none() => {
                let unit = seg[7];
                let x = i32::from(u16::from_be_bytes([seg[8], seg[9]]));
                let y = i32::from(u16::from_be_bytes([seg[10], seg[11]]));
                match unit {
                    1 => j.res = Some((x, y)),
                    2 => j.res = Some(((f64::from(x) * 2.54 + 0.5) as i32, (f64::from(y) * 2.54 + 0.5) as i32)),
                    _ => {}
                }
            }
            0xEE if seg.starts_with(b"Adobe") => j.adobe = true,
            0xC0..=0xCF if m != 0xC4 && m != 0xC8 && m != 0xCC => {
                if seg.len() < 6 {
                    return Err("reading JPG image failed (bad SOF)".into());
                }
                j.bits = seg[0];
                j.height = u16::from_be_bytes([seg[1], seg[2]]);
                j.width = u16::from_be_bytes([seg[3], seg[4]]);
                j.comps = seg[5];
                if m == 0xC3 || m >= 0xC5 && m != 0xC9 && m != 0xCA {
                    // lossless / hierarchical / arithmetic: pdfTeX refuses the non-DCT kinds
                    if m == 0xC3 || m == 0xC7 || m == 0xCB || m == 0xCF {
                        return Err("unsupported type of compression (lossless JPEG)".into());
                    }
                }
                return Ok(j);
            }
            _ => {}
        }
        at += len;
    }
}

fn jpeg_info(b: &[u8]) -> ImageInfo {
    match jpeg_scan(b) {
        Err(e) => ImageInfo::err(Kind::Jpeg, e),
        Ok(j) => {
            let mut i = ImageInfo::empty(Kind::Jpeg);
            i.x_size = i32::from(j.width);
            i.y_size = i32::from(j.height);
            i.bits = j.bits;
            i.colors = j.comps;
            if let Some((x, y)) = j.res {
                i.x_res = x;
                i.y_res = y;
            }
            if !matches!(j.comps, 1 | 3 | 4) {
                i.error = Some("Unsupported color space".into());
            }
            i
        }
    }
}

fn jpeg_include(w: &mut dyn ObjSink, b: &[u8]) -> Result<u32, String> {
    let j = jpeg_scan(b)?;
    let num = w.alloc();
    let mut d = head(u32::from(j.width), u32::from(j.height), j.bits);
    match j.comps {
        1 => d.push_str("/ColorSpace /DeviceGray\n"),
        3 => d.push_str("/ColorSpace /DeviceRGB\n"),
        4 => {
            d.push_str("/ColorSpace /DeviceCMYK\n");
            if j.adobe {
                d.push_str("/Decode [1 0 1 0 1 0 1 0]\n");
            }
        }
        _ => return Err("Unsupported color space".into()),
    }
    d.push_str("/Filter /DCTDecode\n");
    w.write_obj(num, &stream_obj(&d, b));
    Ok(num)
}

// ---------------------------------------------------------------- PDF

fn open_pdf(b: &[u8]) -> Result<pdfread::Doc, String> {
    let data: Arc<[u8]> = Arc::from(b);
    pdfread::Doc::open(&data).map_err(|e| match e {
        pdfread::Error::Encrypted => "PDF inclusion: encrypted PDF files are not supported".to_string(),
        pdfread::Error::Broken => "PDF inclusion: reading image failed".to_string(),
    })
}

fn chosen_box(page: &pdfread::Page, req: &Request, p: &Params) -> [f64; 4] {
    let which = if req.page_box > 0 { req.page_box } else { p.page_box };
    match which {
        1 => page.media,
        3 => page.bleed,
        4 => page.trim,
        5 => page.art,
        _ => page.crop,
    }
}

fn version_check(doc: &pdfread::Doc, p: &Params, i: &mut ImageInfo) {
    let minor = ((doc.version - 1.0) * 10.0 + 0.5) as i32;
    if doc.version >= 1.0 && minor > p.pdf_minor {
        let msg = format!(
            "PDF inclusion: found PDF version <{:.1}>, but at most version <1.{}> allowed",
            doc.version, p.pdf_minor
        );
        if p.inclusion_errorlevel > 0 {
            i.error = Some(msg);
        } else if p.inclusion_errorlevel == 0 {
            i.warnings.push(msg);
        }
    }
}

fn pdf_info(b: &[u8], req: &Request, p: &Params) -> ImageInfo {
    let doc = match open_pdf(b) {
        Ok(d) => d,
        Err(e) => return ImageInfo::err(Kind::Pdf, e),
    };
    let mut i = ImageInfo::empty(Kind::Pdf);
    version_check(&doc, p, &mut i);
    let n = doc.num_pages() as i32;
    i.pages = n;
    if req.page <= 0 || req.page > n {
        i.error = Some(format!("PDF inclusion: required page <{}> does not exist", req.page));
        return i;
    }
    i.page = req.page;
    let Some(page) = doc.page(req.page as usize) else {
        i.error = Some(format!("PDF inclusion: required page <{}> does not exist", req.page));
        return i;
    };
    let bx = chosen_box(&page, req, p);
    let (x1, y1) = (bx[0].min(bx[2]), bx[1].min(bx[3]));
    let (x2, y2) = (bx[0].max(bx[2]), bx[1].max(bx[3]));
    i.rotate = page.rotate.rem_euclid(360);
    let (bw, bh) = (bp2sp(x2 - x1), bp2sp(y2 - y1));
    i.x_size = (x2 - x1).round() as i32;
    i.y_size = (y2 - y1).round() as i32;
    if i.rotate == 90 || i.rotate == 270 {
        i.width = bh;
        i.height = bw;
    } else {
        i.width = bw;
        i.height = bh;
    }
    if i.rotate == 0 {
        i.orig_x = bp2sp(x1);
        i.orig_y = bp2sp(y1);
    }
    i.bits = 0;
    i
}

/// Copies objects of the included file into the output, renumbered.
struct Copier<'a> {
    doc: &'a pdfread::Doc,
    map: HashMap<Ref, u32>,
    todo: Vec<Ref>,
    digits: usize,
}

impl Copier<'_> {
    fn num(&mut self, w: &mut dyn ObjSink, r: Ref) -> u32 {
        if let Some(&n) = self.map.get(&r) {
            return n;
        }
        let n = w.alloc();
        self.map.insert(r, n);
        self.todo.push(r);
        n
    }

    fn ser(&mut self, w: &mut dyn ObjSink, o: &Obj, out: &mut Vec<u8>) {
        match o {
            Obj::Null | Obj::Cmd(_) | Obj::Error | Obj::Eof => out.extend_from_slice(b"null"),
            Obj::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
            Obj::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
            Obj::Real(r) => out.extend_from_slice(real(*r, self.digits).as_bytes()),
            Obj::Str(s) => {
                out.push(b'<');
                for c in s {
                    out.extend_from_slice(format!("{c:02x}").as_bytes());
                }
                out.push(b'>');
            }
            Obj::Name(n) => name(n, out),
            Obj::Array(a) => {
                out.push(b'[');
                for (k, x) in a.iter().enumerate() {
                    if k > 0 {
                        out.push(b' ');
                    }
                    self.ser(w, x, out);
                }
                out.push(b']');
            }
            Obj::Dict(d) => self.dict(w, d, out, false),
            Obj::Stream(s) => self.dict(w, &s.dict, out, true),
            Obj::Ref(r) => {
                let n = self.num(w, *r);
                out.extend_from_slice(format!("{n} 0 R").as_bytes());
            }
        }
    }

    /// A dictionary; a stream's without its `/Length` (written by the caller).
    fn dict(&mut self, w: &mut dyn ObjSink, d: &Dict, out: &mut Vec<u8>, stream: bool) {
        out.extend_from_slice(b"<<");
        for (k, v) in &d.0 {
            if stream && k == b"Length" {
                continue;
            }
            out.push(b'\n');
            name(k, out);
            out.push(b' ');
            self.ser(w, v, out);
        }
        out.extend_from_slice(b"\n>>");
    }

    /// Writes every object referenced so far, and those they reference.
    fn drain(&mut self, w: &mut dyn ObjSink) {
        while let Some(r) = self.todo.pop() {
            let n = self.map[&r];
            let o = self.doc.fetch(r);
            let mut body = Vec::new();
            if let Obj::Stream(s) = &o {
                let raw = self.doc.raw(s).to_vec();
                let mut d = Vec::new();
                self.dict(w, &s.dict, &mut d, true);
                d.truncate(d.len() - 2);
                body.extend_from_slice(&d);
                body.extend_from_slice(format!("/Length {}\n>>\nstream\n", raw.len()).as_bytes());
                body.extend_from_slice(&raw);
                body.extend_from_slice(b"\nendstream");
            } else {
                self.ser(w, &o, &mut body);
            }
            w.write_obj(n, &body);
        }
    }
}

fn name(n: &[u8], out: &mut Vec<u8>) {
    out.push(b'/');
    for &c in n {
        if c <= b' ' || c >= 0x7F || b"#()<>[]{}/%".contains(&c) {
            out.extend_from_slice(format!("#{c:02X}").as_bytes());
        } else {
            out.push(c);
        }
    }
}

/// A real with at most `digits` decimals, trailing zeros dropped.
fn real(x: f64, digits: usize) -> String {
    let mut s = format!("{x:.digits$}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if s == "-0" { "0".into() } else { s }
}

fn pdf_include(w: &mut dyn ObjSink, file: &str, b: &[u8], req: &Request, p: &Params) -> Result<u32, String> {
    let doc = open_pdf(b)?;
    let page = doc
        .page(req.page.max(1) as usize)
        .ok_or_else(|| format!("PDF inclusion: required page <{}> does not exist", req.page))?;
    let pd = match doc.fetch(page.dict_ref) {
        Obj::Dict(d) => d,
        _ => return Err("PDF inclusion: reading image failed".into()),
    };
    let digits = 8;
    let mut c = Copier { doc: &doc, map: HashMap::new(), todo: Vec::new(), digits };
    let num = w.alloc();
    let bx = chosen_box(&page, req, p);
    let (x1, y1) = (bx[0].min(bx[2]), bx[1].min(bx[3]));
    let (x2, y2) = (bx[0].max(bx[2]), bx[1].max(bx[3]));

    let mut d = Vec::new();
    d.extend_from_slice(b"/Type /XObject\n/Subtype /Form\n");
    if p.suppress_ptex_info & 1 == 0 {
        d.extend_from_slice(b"/PTEX.FileName ");
        c.ser(w, &Obj::Str(file.as_bytes().to_vec()), &mut d);
        d.push(b'\n');
    }
    if p.suppress_ptex_info & 2 == 0 {
        d.extend_from_slice(format!("/PTEX.PageNumber {}\n", req.page.max(1)).as_bytes());
    }
    if p.suppress_ptex_info & 4 == 0 {
        if let Some(info @ Obj::Ref(_)) = doc.trailer.get(b"Info") {
            d.extend_from_slice(b"/PTEX.InfoDict ");
            c.ser(w, info, &mut d);
            d.push(b'\n');
        }
    }
    let r = |x: f64| real(x, digits);
    d.extend_from_slice(format!("/BBox [{} {} {} {}]\n", r(x1), r(y1), r(x2), r(y2)).as_bytes());
    // the rotation, the box moved to the origin
    let m = match page.rotate.rem_euclid(360) {
        90 => Some([0.0, -1.0, 1.0, 0.0, -y1, x2]),
        180 => Some([-1.0, 0.0, 0.0, -1.0, x2, y2]),
        270 => Some([0.0, 1.0, -1.0, 0.0, y2, -x1]),
        _ => None,
    };
    if let Some(m) = m {
        d.extend_from_slice(
            format!("/Matrix [{} {} {} {} {} {}]\n", r(m[0]), r(m[1]), r(m[2]), r(m[3]), r(m[4]), r(m[5])).as_bytes(),
        );
    }
    for key in [&b"Group"[..], b"Metadata", b"PieceInfo", b"SeparationInfo"] {
        if let Some(v) = pd.get(key) {
            name(key, &mut d);
            d.push(b' ');
            c.ser(w, v, &mut d);
            d.push(b'\n');
        }
    }
    if let Some(res) = &page.resources {
        d.extend_from_slice(b"/Resources ");
        c.dict(w, res, &mut d, false);
        d.push(b'\n');
    }
    // the contents: one stream copied as it is, else the decoded parts joined
    let contents = doc.lookup(&pd, b"Contents");
    let data = match &contents {
        Obj::Stream(s) => {
            for key in [&b"Filter"[..], b"DecodeParms"] {
                if let Some(v) = s.dict.get(key) {
                    name(key, &mut d);
                    d.push(b' ');
                    c.ser(w, v, &mut d);
                    d.push(b'\n');
                }
            }
            doc.raw(s).to_vec()
        }
        Obj::Array(parts) => {
            let mut all = Vec::new();
            for part in parts {
                if let Obj::Stream(s) = doc.follow(part) {
                    all.extend_from_slice(&doc.decode(&s));
                    all.push(b'\n');
                }
            }
            let (f, data) = compress(&all, p.compress_level);
            d.extend_from_slice(f.as_bytes());
            data
        }
        _ => Vec::new(),
    };
    let mut body = b"<<\n".to_vec();
    body.extend_from_slice(&d);
    body.extend_from_slice(format!("/Length {}\n>>\nstream\n", data.len()).as_bytes());
    body.extend_from_slice(&data);
    body.extend_from_slice(b"\nendstream");
    w.write_obj(num, &body);
    c.drain(w);
    Ok(num)
}

// ---------------------------------------------------------------- include

/// Writes the image `bytes` (from file `file`) and returns the object
/// number of its XObject. `Err` carries pdfTeX's error message.
pub fn include(w: &mut dyn ObjSink, file: &str, bytes: &[u8], req: &Request, p: &Params) -> Result<u32, String> {
    match kind_of(bytes) {
        Kind::Png => png_include(w, bytes, p),
        Kind::Jpeg => jpeg_include(w, bytes),
        Kind::Pdf => pdf_include(w, file, bytes, req, p),
        Kind::Jbig2 => Err("JBIG2 images are not supported".into()),
        Kind::Unknown => Err("unknown graphics file type".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        // 72 dpi: one pixel is one bp
        assert_eq!(ext_xn_over_d(ONE_HUNDRED_INCH, 72, 7200), 4736287);
        assert_eq!(real(1.50, 4), "1.5");
        assert_eq!(real(-0.0, 4), "0");
    }
}
