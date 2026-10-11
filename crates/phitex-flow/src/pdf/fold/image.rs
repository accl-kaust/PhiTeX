//! The fold's images (pdftex.web's `pdf_write_image`, writeimg.c's
//! `write_image`): a JPEG's data as they are, a PNG (writepng.c's
//! `write_png`: its IDAT data copied when PDF can decode them, else its
//! rows as libpng gives them, an alpha channel as a soft mask, a palette
//! as an `/Indexed` colour space), a PDF file's page (pdftoepdf.cc's
//! `write_epdf`: a form, the objects it refers to copied after it under
//! new numbers, its Type 1 fonts the map has replaced by pdfTeX's own
//! embedding). Copied from partex-core's `pdf/image.rs`,
//! `pdf/writepng.rs` and `pdf/epdf.rs`, with the engine taken out.

use std::sync::Arc;

use super::{Data, F, Out};
use crate::pdf::image::{Image, JPG_GRAY, JPG_RGB, PdfImage};
use crate::pdf::lib::pdfread::{Dict, Doc, Obj, Ref, Stream};
use crate::pdf::lib::png::{self, Info, Transforms};

/// What an object copied is (`InObjType`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum InKind {
    /// `objOther`: copied as it is.
    Other,
    /// `objFont`: a font replaced: its descriptor's key and its
    /// encoding's object.
    Font { fd: (Vec<u8>, i32, i32), enc: u32 },
    /// `objFontDesc`: a replaced font's descriptor (the end writes it).
    FontDesc,
}

/// An object of an included file copied (`InObj`).
#[derive(Clone, Debug)]
pub(super) struct InObj {
    num: i32,
    generation: i32,
    objnum: u32,
    written: bool,
    kind: InKind,
}

/// png.h's colour types.
const GRAY: u8 = 0;
const RGB: u8 = 2;
const PALETTE: u8 = 3;
const GRAY_ALPHA: u8 = 4;
const RGB_ALPHA: u8 = 6;
const COLOR_MASK_ALPHA: u8 = 4;

/// png.h's `PNG_INFO_*` bits.
const INFO_GAMA: u32 = 0x0001;
const INFO_SBIT: u32 = 0x0002;
const INFO_CHRM: u32 = 0x0004;
const INFO_TRNS: u32 = 0x0010;
const INFO_BKGD: u32 = 0x0020;
const INFO_HIST: u32 = 0x0040;
const INFO_SRGB: u32 = 0x0800;
const INFO_ICCP: u32 = 0x1000;
const INFO_SPLT: u32 = 0x2000;

/// `PNG_FP_1`.
const FP_1: i32 = 100_000;

/// `SPNG_CHUNK_IDAT`, `SPNG_CHUNK_IEND`.
const IDAT: i32 = 0x4944_4154;
const IEND: i32 = 0x4945_4E44;

fn getint(d: &[u8], pos: i64) -> Option<i32> {
    let p = usize::try_from(pos).ok()?;
    let b = d.get(p..p + 4)?;
    Some(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
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
    out.extend_from_slice(format!("{ival}").as_bytes());
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

/// utils.c's `stripzeros`.
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
    let mut v = Vec::new();
    for (i, x) in xs.iter().enumerate() {
        if i > 0 {
            v.push(b' ');
        }
        v.extend_from_slice(format!("{x:.8}").as_bytes());
    }
    v
}

/// utils.c's `convertStringToPDFString`.
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

fn page_box(p: &crate::pdf::lib::pdfread::Page, spec: i32) -> Option<[f64; 4]> {
    crate::pdf::image::page_box(p, spec)
}

/// A document's copy in progress.
struct Copy<'d> {
    doc: &'d Doc,
    objs: Vec<InObj>,
    /// The replaced fonts' encodings (`None`: a CID font) and their
    /// objects, the last added first.
    encodings: Vec<(Option<crate::pdf::lib::fofi::Encoding>, u32)>,
}

/// What `copyFont` reads of a font it replaces.
struct Replaced {
    fm: Arc<crate::pdf::font::map::MapEntry>,
    desc: Ref,
    stem_v: f64,
    charset: Option<Vec<u8>>,
    dict: Dict,
}

/// A fatal error of an image's (`pdftex_fail`).
struct Fail(String);

impl F<'_> {
    /// `pdf_end_stream` after bytes written as they are (no length to
    /// write over).
    fn end_stream_raw(&mut self) {
        self.out.push(b'\n');
        self.out.extend_from_slice(b"endstream\n");
        self.end_obj();
    }

    fn fail(&mut self, file: &[u8], m: &str) {
        self.errors.push(format!("!pdfTeX error: pdftex (file {}): {m}", String::from_utf8_lossy(file)));
    }

    /// `pdf_write_image`.
    pub(super) fn write_image(&mut self, n: u32) {
        let Some(Data::Ximage { image, attr, .. }) = self.data.get(&n).cloned() else { return };
        self.begin_dict(n, 0);
        if let Some(a) = attr.filter(|a| !a.is_empty()) {
            self.print_ln(&a);
        }
        let draft = self.version.as_ref().is_some_and(|v| v.draftmode != 0);
        if !draft {
            let r = if let Some(p) = &image.pdf {
                self.write_epdf(&image, p)
            } else if image.png {
                self.write_png(&image)
            } else {
                self.write_jpg(&image);
                Ok(())
            };
            if let Err(Fail(m)) = r {
                self.fail(&image.name, &m);
            }
        }
        if let Some(Data::Ximage { attr, .. }) = self.data.get_mut(&n) {
            *attr = None;
        }
        // (`delete_image`: the document closed when no image of it waits)
        if image.pdf.is_some()
            && let Some(i) = self.epdf.iter().position(|d| d.name == image.name)
            && self.epdf[i].occurrences < 0
        {
            self.epdf.remove(i);
        }
    }

    /// writejpg.c's `write_jpg`.
    fn write_jpg(&mut self, image: &Image) {
        let mut o = Out::new();
        o.print_ln(b"/Type /XObject");
        o.print_ln(b"/Subtype /Image");
        o.int_entry_ln(b"Width", i64::from(image.width));
        o.int_entry_ln(b"Height", i64::from(image.height));
        o.int_entry_ln(b"BitsPerComponent", i64::from(image.bits));
        o.int_entry_ln(b"Length", image.data.len() as i64);
        o.print(b"/ColorSpace ");
        if image.colorspace_ref == 0 {
            match image.color_space {
                JPG_GRAY => o.print_ln(b"/DeviceGray"),
                JPG_RGB => o.print_ln(b"/DeviceRGB"),
                _ => {
                    o.print_ln(b"/DeviceCMYK");
                    o.print_ln(b"/Decode [1 0 1 0 1 0 1 0]");
                }
            }
        } else {
            o.int(i64::from(image.colorspace_ref));
            o.print_ln(b" 0 R");
        }
        o.print_ln(b"/Filter /DCTDecode");
        o.print_ln(b">>");
        o.print_ln(b"stream");
        self.print_out(&o);
        self.out.extend_from_slice(&image.data);
        self.end_stream_raw();
    }

    fn png_colorspace(o: &mut Out, colorspace_ref: i32, gray: bool) {
        if colorspace_ref != 0 {
            o.int(i64::from(colorspace_ref));
            o.print_ln(b" 0 R");
        } else if gray {
            o.print_ln(b"/DeviceGray");
        } else {
            o.print_ln(b"/DeviceRGB");
        }
    }

    /// writepng.c's `write_png`, the image's dictionary begun.
    #[allow(clippy::too_many_lines)]
    fn write_png(&mut self, image: &Image) -> Result<(), Fail> {
        let Ok(info) = png::read_info(&image.data) else {
            return Err(Fail("libpng: internal error".into()));
        };
        let (major, minor) = self.fixed_version();
        let mut hicolor = self.version.as_ref().map_or(0, |v| v.image_hicolor);
        if major == 1 && minor < 5 {
            hicolor = 0;
        }
        let apply_gamma = self.version.as_ref().map_or(0, |v| v.image_apply_gamma);
        let mut o = Out::new();
        o.print_ln(b"/Type /XObject");
        o.print_ln(b"/Subtype /Image");
        let mut copy = true;
        let mut t = Transforms::default();
        if info.get_valid(INFO_TRNS) != 0 {
            t.trns_to_alpha = true;
            copy = false;
        }
        if major == 1 && minor < 4 && info.color_type & COLOR_MASK_ALPHA != 0 {
            t.strip_alpha = true;
            copy = false;
        }
        if info.bit_depth == 16 && hicolor == 0 {
            t.strip_16 = true;
            copy = false;
        }
        let file_gamma = (info.get_valid(INFO_GAMA) != 0).then_some(info.gamma);
        if apply_gamma != 0 {
            return Err(Fail("\\pdfimageapplygamma is not supported yet".into()));
        }
        let (color_type, bit_depth) = png::output(&info, t);
        o.int_entry_ln(b"Width", i64::from(info.width));
        o.int_entry_ln(b"Height", i64::from(info.height));
        o.int_entry_ln(b"BitsPerComponent", i64::from(bit_depth));
        o.print(b"/ColorSpace ");
        let others = INFO_CHRM | INFO_ICCP | INFO_SBIT | INFO_SRGB | INFO_BKGD | INFO_HIST | INFO_TRNS | INFO_SPLT;
        if copy
            && (major > 1 || minor > 1)
            && info.interlace == 0
            && matches!(color_type, GRAY | RGB)
            && file_gamma.is_none_or(|g| g == FP_1)
            && info.get_valid(others) == 0
        {
            Self::png_colorspace(&mut o, image.colorspace_ref, color_type == GRAY);
            self.print_out(&o);
            return self.copy_png(image, &info, bit_depth);
        }
        let Ok(img) = png::read_image(&image.data, &info, t) else {
            return Err(Fail("libpng: internal error".into()));
        };
        let rows = &img.rows;
        let mut needs_group = false;
        match img.color_type {
            PALETTE => {
                let palette_objnum = self.new_objnum();
                if image.colorspace_ref != 0 {
                    Self::png_colorspace(&mut o, image.colorspace_ref, false);
                } else {
                    o.print(b"[/Indexed /DeviceRGB ");
                    o.int(info.palette.len() as i64 - 1);
                    o.print(b" ");
                    o.int(i64::from(palette_objnum));
                    o.print_ln(b" 0 R]");
                }
                self.print_out(&o);
                self.stream_raw(rows);
                self.begin_dict(palette_objnum, 0);
                let bytes: Vec<u8> = info.palette.iter().flatten().copied().collect();
                self.stream_raw(&bytes);
            }
            GRAY | RGB => {
                Self::png_colorspace(&mut o, image.colorspace_ref, img.color_type == GRAY);
                self.print_out(&o);
                self.stream_raw(rows);
            }
            GRAY_ALPHA | RGB_ALPHA if major > 1 || minor >= 4 => {
                let gray = img.color_type == GRAY_ALPHA;
                Self::png_colorspace(&mut o, image.colorspace_ref, gray);
                let smask_objnum = self.new_objnum();
                o.print(b"/SMask ");
                o.int(i64::from(smask_objnum));
                o.print_ln(b" 0 R");
                self.print_out(&o);
                let wide = img.bit_depth == 16 && hicolor != 0;
                let (n, alpha_from) = match (gray, wide) {
                    (true, true) => (4, 2),
                    (true, false) => (2, 1),
                    (false, true) => (8, 6),
                    (false, false) => (4, 3),
                };
                let mut color = Vec::with_capacity(rows.len());
                let mut smask = Vec::with_capacity(rows.len() / n * (n - alpha_from));
                for row in rows.chunks(img.rowbytes.max(1)) {
                    for (j, &b) in row.iter().enumerate() {
                        if j % n < alpha_from {
                            color.push(b);
                        } else {
                            smask.push(b);
                        }
                    }
                }
                self.stream_raw(&color);
                let depth = img.bit_depth;
                self.begin_dict(smask_objnum, 0);
                let mut o = Out::new();
                o.print_ln(b"/Type /XObject");
                o.print_ln(b"/Subtype /Image");
                o.int_entry_ln(b"Width", i64::from(info.width));
                o.int_entry_ln(b"Height", i64::from(info.height));
                o.int_entry_ln(b"BitsPerComponent", if depth == 16 { 8 } else { i64::from(depth) });
                o.print_ln(b"/ColorSpace /DeviceGray");
                self.print_out(&o);
                let mask: Vec<u8> = if depth == 16 { smask.iter().step_by(2).copied().collect() } else { smask };
                self.stream_raw(&mask);
                needs_group = true;
            }
            GRAY_ALPHA | RGB_ALPHA => {
                Self::png_colorspace(&mut o, image.colorspace_ref, img.color_type == GRAY_ALPHA);
                self.print_out(&o);
                self.stream_raw(rows);
            }
            t => return Err(Fail(format!("unsupported type of color_type <{t}>"))),
        }
        // (`write_additional_png_objects`)
        if needs_group && !self.png_group_written && self.png_group > 0 {
            self.png_group_written = true;
            let g = self.png_group;
            self.begin_obj(g, 2);
            if self.compress_level == 0 {
                self.print_ln(b"%PTEX Group needed for transparent pngs");
            }
            self.print_ln(b"<</Type/Group /S/Transparency /CS/DeviceRGB /I true>>");
            self.end_obj();
        }
        Ok(())
    }

    /// `copy_png`: the IDAT chunks' data as they are.
    fn copy_png(&mut self, image: &Image, info: &Info, depth: u8) -> Result<(), Fail> {
        let d = &image.data[..];
        let mut pos: i64 = 8;
        let mut length: i32 = 0;
        loop {
            let (Some(len), Some(kind)) = (getint(d, pos), getint(d, pos + 4)) else {
                return Err(Fail("writepng: reading chunk type failed".into()));
            };
            pos += 8;
            if kind == IEND {
                break;
            }
            if kind == IDAT {
                length = length.wrapping_add(len);
            }
            pos += i64::from(len) + 4;
            if pos < 0 {
                return Err(Fail("writepng: fseek in PNG file failed".into()));
            }
        }
        let colors = if info.color_type == RGB { 3 } else { 1 };
        let mut o = Out::new();
        o.print(b"/Length ");
        o.int(i64::from(length));
        o.print(b"\n/Filter/FlateDecode\n/DecodeParms<</Colors ");
        o.int(colors);
        o.print(b"/Columns ");
        o.int(i64::from(info.width));
        o.print(b"/BitsPerComponent ");
        o.int(i64::from(depth));
        o.print(b"/Predictor 10>>\n>>\nstream\n");
        self.print_out(&o);
        let mut pos: i64 = 8;
        let mut idat = 0;
        loop {
            let (Some(len), Some(kind)) = (getint(d, pos), getint(d, pos + 4)) else {
                return Err(Fail("writepng: reading chunk type failed".into()));
            };
            pos += 8;
            match kind {
                IDAT => {
                    if idat == 2 {
                        return Err(Fail("writepng: IDAT chunk sequence broken".into()));
                    }
                    idat = 1;
                    let start = usize::try_from(pos).unwrap_or(usize::MAX).min(d.len());
                    let n = usize::try_from(len.max(0)).unwrap_or(0);
                    self.out.extend_from_slice(&d[start..(start + n).min(d.len())]);
                    pos += i64::from(len.max(0)) + 4;
                }
                IEND => {
                    self.end_stream_raw();
                    return Ok(());
                }
                _ => {
                    if idat == 1 {
                        idat = 2;
                    }
                    pos += i64::from(len) + 4;
                    if pos < 0 {
                        return Err(Fail("writepng: fseek in PNG file failed".into()));
                    }
                }
            }
        }
    }

    // ---- pdftoepdf.cc

    /// `pdf_newline`: a newline unless the last byte written is one.
    fn ep_newline(&mut self) {
        if self.buf().last() != Some(&b'\n') {
            self.puts(b"\n");
        }
    }

    fn ep_ref(&mut self, n: u32) {
        self.puts(format!("{n} 0 R").as_bytes());
    }

    /// `copyName`.
    fn ep_name(&mut self, s: &[u8]) {
        self.puts(b"/");
        for &c in s {
            if c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-' | b'+') {
                self.puts(&[c]);
            } else {
                self.puts(format!("#{c:02X}").as_bytes());
            }
        }
    }

    /// `addInObj`: object `r` as `kind` (numbered `num` or anew), or its
    /// number if it is there already.
    fn ep_add(&mut self, c: &mut Copy<'_>, r: Ref, kind: InKind, num: Option<u32>) -> Result<u32, Fail> {
        if r.num == 0 {
            return Err(Fail("PDF inclusion: invalid reference".into()));
        }
        if let Some(o) = c.objs.iter().find(|o| o.num == r.num && o.generation == r.generation) {
            return Ok(o.objnum);
        }
        let objnum = match num {
            Some(n) => n,
            None => self.new_objnum(),
        };
        c.objs.push(InObj { num: r.num, generation: r.generation, objnum, written: false, kind });
        Ok(objnum)
    }

    fn ep_dict(&mut self, c: &mut Copy<'_>, d: &Dict) -> Result<(), Fail> {
        for (k, v) in &d.0 {
            self.ep_name(k);
            self.puts(b" ");
            self.ep_object(c, v)?;
            self.puts(b"\n");
        }
        Ok(())
    }

    /// `copyObject`.
    fn ep_object(&mut self, c: &mut Copy<'_>, o: &Obj) -> Result<(), Fail> {
        match o {
            Obj::Bool(b) => self.puts(if *b { b"true" } else { b"false" }),
            Obj::Int(i) => self.puts(format!("{i}").as_bytes()),
            Obj::Real(r) => self.puts(&num_to_pdf(*r)),
            Obj::Str(s) => {
                if s.contains(&0) {
                    self.puts(b"<");
                    for &b in s {
                        self.puts(format!("{b:02x}").as_bytes());
                    }
                    self.puts(b">");
                } else {
                    self.puts(b"(");
                    for &b in s {
                        if b == b'(' || b == b')' || b == b'\\' {
                            self.puts(&[b'\\', b]);
                        } else if !(0x20..=0x7f).contains(&b) {
                            self.puts(format!("\\{b:03o}").as_bytes());
                        } else {
                            self.puts(&[b]);
                        }
                    }
                    self.puts(b")");
                }
            }
            Obj::Name(n) => self.ep_name(n),
            Obj::Null => self.puts(b"null"),
            Obj::Array(a) => {
                self.puts(b"[");
                for x in a {
                    if !matches!(x, Obj::Name(_)) {
                        self.puts(b" ");
                    }
                    self.ep_object(c, x)?;
                }
                self.puts(b"]");
            }
            Obj::Dict(d) => {
                self.puts(b"<<\n");
                self.ep_dict(c, d)?;
                self.puts(b">>");
            }
            Obj::Stream(s) => {
                self.puts(b"<<\n");
                self.ep_dict(c, &s.dict)?;
                self.puts(b">>\n");
                self.puts(b"stream\n");
                let raw = c.doc.raw(s).to_vec();
                self.puts(&raw);
                self.puts(b"\nendstream");
            }
            Obj::Ref(r) => {
                if r.num == 0 {
                    return Err(Fail("PDF inclusion: reference to invalid object (is the included pdf broken?)".into()));
                }
                let n = self.ep_add(c, *r, InKind::Other, None)?;
                self.ep_ref(n);
            }
            _ => return Err(Fail(format!("PDF inclusion: type <{}> cannot be copied", o.type_name()))),
        }
        Ok(())
    }

    /// `copyFontResources`.
    fn ep_fonts(&mut self, c: &mut Copy<'_>, o: &Obj) -> Result<(), Fail> {
        let Obj::Dict(d) = o else {
            return Err(Fail(format!("PDF inclusion: invalid font resources dict type <{}>", o.type_name())));
        };
        self.puts(b"/Font << ");
        for (k, v) in &d.0 {
            match v {
                Obj::Ref(r) => {
                    if let Some(o) = c.objs.iter().find(|o| o.num == r.num && o.generation == r.generation) {
                        let n = o.objnum;
                        self.ep_name(k);
                        self.puts(b" ");
                        self.ep_ref(n);
                        self.puts(b" ");
                    } else if let Some(rep) = self.ep_replaceable(c, *r) {
                        self.ep_replace_font(c, k, *r, &rep)?;
                    } else {
                        self.ep_name(k);
                        self.puts(b" ");
                        self.ep_object(c, v)?;
                    }
                }
                Obj::Dict(_) => {
                    self.ep_name(k);
                    self.puts(b" ");
                    self.ep_object(c, v)?;
                }
                _ => return Err(Fail(format!("PDF inclusion: invalid font in reference type <{}>", v.type_name()))),
            }
        }
        self.puts(b">>\n");
        Ok(())
    }

    /// `copyFont`'s test.
    fn ep_replaceable(&mut self, c: &Copy<'_>, r: Ref) -> Option<Replaced> {
        if self.version.as_ref().is_some_and(|v| v.copy_fonts != 0) {
            return None;
        }
        let doc = c.doc;
        let Obj::Dict(dict) = doc.fetch(r) else { return None };
        if doc.lookup(&dict, b"Subtype").as_name() != Some(b"Type1") {
            return None;
        }
        let Obj::Name(base) = doc.lookup(&dict, b"BaseFont") else { return None };
        let Some(Obj::Ref(desc)) = dict.get(b"FontDescriptor") else { return None };
        let desc = *desc;
        let Obj::Dict(fd) = doc.fetch(desc) else { return None };
        let file_ok = matches!(doc.lookup(&fd, b"FontFile"), Obj::Stream(_))
            || matches!(doc.lookup(&fd, b"FontFile3"),
                Obj::Stream(s) if doc.lookup(&s.dict, b"Subtype").as_name() == Some(b"Type1C"));
        if !file_ok {
            return None;
        }
        let fm = self.map_lookup_ps(&base)?;
        let stem_v = doc.lookup(&fd, b"StemV").as_num().unwrap_or(0.0);
        let charset = match doc.lookup(&fd, b"CharSet") {
            Obj::Str(s) => Some(s),
            _ => None,
        };
        Some(Replaced { fm, desc, stem_v, charset, dict })
    }

    /// `copyFont`'s replacement.
    fn ep_replace_font(&mut self, c: &mut Copy<'_>, tag: &[u8], r: Ref, rep: &Replaced) -> Result<(), Fail> {
        let key = self.epdf_fontdescriptor(&rep.fm, crate::show::zround(rep.stem_v));
        match &rep.charset {
            Some(cs) if rep.fm.is(crate::pdf::font::map::F_SUBSETTED) => self.epdf_mark_glyphs(&key, cs),
            _ => {
                if let Some(fd) = self.fd_tree.get_mut(&key) {
                    fd.all_glyphs = true;
                }
            }
        }
        let fd_objnum = self.fd_tree.get(&key).map_or(0, |fd| fd.objnum);
        self.ep_add(c, rep.desc, InKind::FontDesc, Some(fd_objnum))?;
        self.ep_name(tag);
        let enc = crate::pdf::lib::gfxfont::encoding(c.doc, &rep.dict);
        let enc_objnum = self.new_objnum();
        c.encodings.insert(0, (enc, enc_objnum));
        let n = self.ep_add(c, r, InKind::Font { fd: key, enc: enc_objnum }, None)?;
        self.puts(b" ");
        self.ep_ref(n);
        self.puts(b" ");
        Ok(())
    }

    /// `copyFontDict`.
    fn ep_font_dict(&mut self, c: &mut Copy<'_>, o: &Obj, fd: &(Vec<u8>, i32, i32), enc: u32) -> Result<(), Fail> {
        let Obj::Dict(d) = o else {
            return Err(Fail(format!("PDF inclusion: invalid dict type <{}>", o.type_name())));
        };
        self.puts(b"<<\n");
        for (k, v) in &d.0 {
            if k.starts_with(b"FontDescriptor") || k.starts_with(b"BaseFont") || k.starts_with(b"Encoding") {
                continue;
            }
            self.ep_name(k);
            self.puts(b" ");
            self.ep_object(c, v)?;
            self.puts(b"\n");
        }
        let fd_objnum = self.fd_tree.get(fd).map_or(0, |x| x.objnum);
        let fn_objnum = self.fn_objnum(fd);
        for (key, n) in [(&b"FontDescriptor"[..], fd_objnum), (b"BaseFont", fn_objnum), (b"Encoding", enc)] {
            self.puts(b"/");
            self.puts(key);
            self.puts(b" ");
            self.puts(format!("{n} 0 R\n").as_bytes());
        }
        self.puts(b">>");
        Ok(())
    }

    /// `writeEncodings`.
    fn ep_write_encodings(&mut self, c: &mut Copy<'_>) -> Result<(), Fail> {
        for (enc, objnum) in std::mem::take(&mut c.encodings) {
            let Some(names) = enc else {
                return Err(Fail(
                    "PDF inclusion: CID fonts are not supported (try to disable font replacement to fix this)".into(),
                ));
            };
            self.begin_dict(objnum, 1);
            self.puts(b"/Type /Encoding\n");
            self.puts(b"/Differences [");
            let mut old: i32 = -2;
            for (i, name) in (0i32..).zip(&names) {
                let Some(name) = name else { continue };
                let mut l = Vec::new();
                if i != old + 1 {
                    if old != -2 {
                        l.push(b' ');
                    }
                    l.extend_from_slice(format!("{i}").as_bytes());
                }
                l.push(b'/');
                l.extend_from_slice(name);
                self.puts(&l);
                old = i;
            }
            self.puts(b"]\n");
            self.end_dict();
        }
        Ok(())
    }

    /// `copyProcSet`.
    fn ep_procset(&mut self, o: &Obj) -> Result<(), Fail> {
        let Obj::Array(a) = o else {
            return Err(Fail(format!("PDF inclusion: invalid ProcSet array type <{}>", o.type_name())));
        };
        self.puts(b"/ProcSet [ ");
        for x in a {
            let Obj::Name(n) = x else {
                return Err(Fail(format!("PDF inclusion: invalid ProcSet entry type <{}>", x.type_name())));
            };
            self.ep_name(n);
            self.puts(b" ");
        }
        self.puts(b"]\n");
        Ok(())
    }

    /// `copyOtherResources`.
    fn ep_other(&mut self, c: &mut Copy<'_>, o: &Obj, key: &[u8]) -> Result<(), Fail> {
        let ok = if key == b"Subtype" { matches!(o, Obj::Name(_)) } else { matches!(o, Obj::Dict(_)) };
        if !ok {
            let what = if key == b"Subtype" {
                "PDF inclusion: Subtype in Resources dict is not a name"
            } else {
                "PDF inclusion: invalid other resource which is no dict"
            };
            self.warnings.push(format!(
                "pdfTeX warning: {} (key '{}', type <{}>); ignored.",
                what,
                String::from_utf8_lossy(key),
                o.type_name()
            ));
            return Ok(());
        }
        self.ep_name(key);
        self.puts(b" ");
        self.ep_object(c, o)
    }

    /// `writeRefs`.
    fn ep_write_refs(&mut self, c: &mut Copy<'_>) -> Result<(), Fail> {
        let mut i = 0;
        while i < c.objs.len() {
            if !c.objs[i].written {
                c.objs[i].written = true;
                let o = c.doc.fetch(Ref { num: c.objs[i].num, generation: c.objs[i].generation });
                match c.objs[i].kind.clone() {
                    InKind::Font { fd, enc } => {
                        self.begin_obj(c.objs[i].objnum, 2);
                        self.ep_font_dict(c, &o, &fd, enc)?;
                        self.puts(b"\n");
                        self.end_obj();
                    }
                    InKind::FontDesc => {}
                    InKind::Other => {
                        let level = if matches!(o, Obj::Stream(_)) { 0 } else { 2 };
                        self.begin_obj(c.objs[i].objnum, level);
                        self.ep_object(c, &o)?;
                        self.puts(b"\n");
                        self.end_obj();
                    }
                }
            }
            i += 1;
        }
        Ok(())
    }

    /// pdftoepdf.cc's `write_epdf`: image `image` (page `im`), its
    /// dictionary begun.
    #[allow(clippy::too_many_lines)]
    fn write_epdf(&mut self, image: &Image, im: &PdfImage) -> Result<(), Fail> {
        let file = image.name.clone();
        let suppress = self.cur.suppress_ptex_info;
        let sep: &[u8] = if self.ptex_use_underscore { b"_" } else { b"." };
        let objs = match self.epdf.iter_mut().find(|d| d.name == file) {
            Some(d) => {
                d.occurrences -= 1;
                std::mem::take(&mut d.objs)
            }
            None => Vec::new(),
        };
        let Ok(doc) = Doc::open(&image.data) else {
            return Err(Fail("xpdf: reading PDF image failed".into()));
        };
        let Some(page) = doc.page(usize::try_from(im.page).unwrap_or(0)) else {
            return Err(Fail("xpdf: reading PDF image failed".into()));
        };
        let mut c = Copy { doc: &doc, objs, encodings: Vec::new() };
        let page_obj = doc.fetch(page.dict_ref);
        let empty = Dict::default();
        let page_dict = page_obj.as_dict().unwrap_or(&empty);
        let rotate = page.rotate;
        self.puts(b"/Type /XObject\n");
        self.puts(b"/Subtype /Form\n");
        self.puts(b"/FormType 1\n");
        if suppress & 0x02 == 0 {
            let mut l = b"/PTEX".to_vec();
            l.extend_from_slice(sep);
            l.extend_from_slice(b"FileName (");
            l.extend_from_slice(&pdf_string(&file));
            l.extend_from_slice(b")\n");
            self.puts(&l);
        }
        if suppress & 0x04 == 0 {
            let mut l = b"/PTEX".to_vec();
            l.extend_from_slice(sep);
            l.extend_from_slice(format!("PageNumber {}\n", im.page).as_bytes());
            self.puts(&l);
        }
        if suppress & 0x08 == 0
            && let Some(Obj::Ref(info)) = doc.trailer.get(b"Info")
        {
            let mut l = b"/PTEX".to_vec();
            l.extend_from_slice(sep);
            l.extend_from_slice(b"InfoDict ");
            self.puts(&l);
            let n = self.ep_add(&mut c, *info, InKind::Other, None)?;
            self.ep_ref(n);
            self.puts(b"\n");
        }
        let Some(b) = page_box(&page, im.page_box) else {
            return Err(Fail(format!("PDF inclusion: unknown value of pagebox spec ({})", im.page_box)));
        };
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
                self.puts(&stripzeros(&l));
            }
        }
        let mut l = b"/BBox [".to_vec();
        l.extend_from_slice(&eight(&b));
        l.extend_from_slice(b"]\n");
        self.puts(&stripzeros(&l));
        if let Some(m) = page_dict.get(b"Metadata")
            && !matches!(m, Obj::Null | Obj::Ref(_))
        {
            self.warnings.push("pdfTeX warning: PDF inclusion: /Metadata must be indirect object".into());
        }
        for key in [&b"LastModified"[..], b"Metadata", b"PieceInfo", b"SeparationInfo"] {
            if let Some(v) = page_dict.get(key)
                && *v != Obj::Null
            {
                self.ep_newline();
                self.puts(b"/");
                self.puts(key);
                self.puts(b" ");
                self.ep_object(&mut c, v)?;
            }
        }
        let mut sep_group = None;
        if let Some(g) = page_dict.get(b"Group")
            && *g != Obj::Null
        {
            let val = self.page_group;
            if val == 0 {
                if self.cur.suppress_pagegroup == 0 {
                    self.warnings.push(
                        "pdfTeX warning: PDF inclusion: multiple pdfs with page group included in a single page".into(),
                    );
                }
                self.ep_newline();
                self.puts(b"/Group ");
                self.ep_object(&mut c, g)?;
            } else {
                let Obj::Dict(gd) = doc.follow(g) else {
                    return Err(Fail("PDF inclusion: /Group dict missing".into()));
                };
                sep_group = Some((val as u32, gd));
                self.puts(b"/Group ");
                self.ep_ref(val as u32);
                self.puts(b"\n");
            }
        }
        match &page.resources {
            None => self.warnings.push(
                "pdfTeX warning: PDF inclusion: /Resources missing. 'This practice is not recommended' (PDF Ref)".into(),
            ),
            Some(res) => {
                self.ep_newline();
                self.puts(b"/Resources <<\n");
                for (k, v) in &res.0 {
                    let v = doc.follow(v);
                    match &k[..] {
                        b"Font" => self.ep_fonts(&mut c, &v)?,
                        b"ProcSet" => self.ep_procset(&v)?,
                        _ => self.ep_other(&mut c, &v, k)?,
                    }
                }
                self.puts(b">>\n");
            }
        }
        match doc.lookup(page_dict, b"Contents") {
            Obj::Stream(s) => self.ep_contents(&doc, &mut c, &s)?,
            Obj::Array(a) => {
                let mut data = Vec::new();
                for (i, x) in a.iter().enumerate() {
                    if let Obj::Stream(s) = doc.follow(x) {
                        data.extend_from_slice(&doc.decode(&s));
                    }
                    if i + 1 < a.len() && data.last() != Some(&b'\n') {
                        data.push(b'\n');
                    }
                }
                self.stream_raw(&data);
            }
            _ => self.stream_raw(&[]),
        }
        self.ep_write_encodings(&mut c)?;
        if let Some((val, gd)) = sep_group {
            self.begin_obj(val, 2);
            self.ep_object(&mut c, &Obj::Dict(gd))?;
            self.puts(b"\n");
            self.end_obj();
            self.page_group = 0;
        }
        self.ep_write_refs(&mut c)?;
        let objs = c.objs;
        if let Some(d) = self.epdf.iter_mut().find(|d| d.name == file) {
            d.objs = objs;
        }
        Ok(())
    }

    /// A page's single content stream as it is in the file.
    fn ep_contents(&mut self, doc: &Doc, c: &mut Copy<'_>, s: &Stream) -> Result<(), Fail> {
        if doc.lookup(&s.dict, b"F") != Obj::Null {
            return Err(Fail("PDF inclusion: Unsupported external stream".into()));
        }
        let len = doc.lookup(&s.dict, b"Length");
        self.puts(b"/Length ");
        self.ep_object(c, &len)?;
        self.puts(b"\n");
        let filter = doc.lookup(&s.dict, b"Filter");
        if filter != Obj::Null {
            self.puts(b"/Filter ");
            self.ep_object(c, &filter)?;
            self.puts(b"\n");
            let parms = doc.lookup(&s.dict, b"DecodeParms");
            if parms != Obj::Null {
                self.puts(b"/DecodeParms ");
                self.ep_object(c, &parms)?;
                self.puts(b"\n");
            }
        }
        self.puts(b">>\nstream\n");
        let raw = doc.raw(s).to_vec();
        self.out.extend_from_slice(&raw);
        self.end_stream_raw();
        Ok(())
    }
}
