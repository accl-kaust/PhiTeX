//! `XeTeX`'s pictures: `\XeTeXpicfile` and `\XeTeXpdffile` (`load_picture`)
//! and `\XeTeXpdfpagecount`. The file is found as `XeTeX_pic.c` finds it
//! (kpathsea's `graphic/figure`) and measured as it measures it: JPEG,
//! BMP and PNG by `image/jpegimage.c`, `bmpimage.c` and `pngimage.c`
//! (libpng's header and `pHYs`), a PDF page's box by `pdfimage.cpp`
//! (pplib: the page tree, the inherited boxes, `/Rotate`). The sizes and
//! the transform keep C's arithmetic: `trans.c`'s transforms in `double`,
//! the corners and the bounds in `float`.

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

use partex_engine::native::PicNode;
use partex_engine::node::{Node, Whatsit};
use partex_engine::pdfread::{Dict, Doc, Obj};

use crate::cmds::MMODE;
use crate::host::{FileKind, Host};
use crate::tex::{Jump, Tex};
use crate::track::Tracker;

/// `pdf_box_type`s: what `find_pic_file` is asked for (0 for
/// `\XeTeXpicfile`).
pub(crate) const PDFBOX_CROP: u8 = 1;
pub(crate) const PDFBOX_MEDIA: u8 = 2;
pub(crate) const PDFBOX_BLEED: u8 = 3;
pub(crate) const PDFBOX_TRIM: u8 = 4;
pub(crate) const PDFBOX_ART: u8 = 5;
pub(crate) const PDFBOX_NONE: u8 = 6;

/// trans.h's `transform`.
#[derive(Clone, Copy)]
struct Transform {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    x: f64,
    y: f64,
}

impl Transform {
    /// `make_identity`.
    fn identity() -> Self {
        Self::scale(1.0, 1.0)
    }

    /// `make_scale`.
    fn scale(xscale: f64, yscale: f64) -> Self {
        Self {
            a: xscale,
            b: 0.0,
            c: 0.0,
            d: yscale,
            x: 0.0,
            y: 0.0,
        }
    }

    /// `make_translation`.
    fn translation(dx: f64, dy: f64) -> Self {
        Self {
            x: dx,
            y: dy,
            ..Self::identity()
        }
    }

    /// `make_rotation`.
    fn rotation(a: f64) -> Self {
        Self {
            a: libm::cos(a),
            b: libm::sin(a),
            c: -libm::sin(a),
            d: libm::cos(a),
            x: 0.0,
            y: 0.0,
        }
    }

    /// `transform_point`: `p` (a `realpoint`, `float`s) moved by `self`.
    #[allow(clippy::cast_possible_truncation, reason = "C stores in a float")]
    fn point(&self, p: &mut Point) {
        let (x, y) = (f64::from(p.0), f64::from(p.1));
        *p = Point(
            (self.a * x + self.c * y + self.x) as f32,
            (self.b * x + self.d * y + self.y) as f32,
        );
    }

    /// `transform_concat`: `self` followed by `t2`.
    fn concat(&mut self, t2: &Self) {
        let t1 = *self;
        *self = Self {
            a: t1.a * t2.a + t1.b * t2.c + 0.0 * t2.x,
            b: t1.a * t2.b + t1.b * t2.d + 0.0 * t2.y,
            c: t1.c * t2.a + t1.d * t2.c + 0.0 * t2.x,
            d: t1.c * t2.b + t1.d * t2.d + 0.0 * t2.y,
            x: t1.x * t2.a + t1.y * t2.c + 1.0 * t2.x,
            y: t1.x * t2.b + t1.y * t2.d + 1.0 * t2.y,
        };
    }
}

/// trans.h's `realpoint` (`float`s).
#[derive(Clone, Copy, Default)]
struct Point(f32, f32);

/// trans.h's `realrect` (`float`s): a picture's bounds in TeX points.
#[derive(Clone, Copy, Default)]
struct Rect {
    x: f32,
    y: f32,
    wd: f32,
    ht: f32,
}

/// C's `(int)v`, as x86-64 converts (a value out of range is `INT_MIN`).
#[allow(clippy::cast_possible_truncation, reason = "checked to fit")]
fn c_int(v: f64) -> i32 {
    if v.is_nan() || v >= 2_147_483_648.0 || v <= -2_147_483_649.0 {
        i32::MIN
    } else {
        v as i32
    }
}

/// `D2Fix`: `(int)(d * 65536.0 + 0.5)`.
fn d2fix(d: f64) -> i32 {
    c_int(d * 65536.0 + 0.5)
}

/// `Fix2D`.
fn fix2d(f: i32) -> f64 {
    f64::from(f) / 65536.0
}

/// `calc_min_and_max`.
fn min_max(corners: &[Point; 4]) -> (f64, f64, f64, f64) {
    let mut xmin = 1_000_000.0;
    let mut xmax = -1_000_000.0; // (`- (integer) xmin`)
    let mut ymin = xmin;
    let mut ymax = xmax;
    for c in corners {
        let (x, y) = (f64::from(c.0), f64::from(c.1));
        if x < xmin {
            xmin = x;
        }
        if x > xmax {
            xmax = x;
        }
        if y < ymin {
            ymin = y;
        }
        if y > ymax {
            ymax = y;
        }
    }
    (xmin, xmax, ymin, ymax)
}

/// `update_corners`.
fn update_corners(corners: &mut [Point; 4], t2: &Transform) {
    for c in corners {
        t2.point(c);
    }
}

/// `do_size_requests`.
fn size_requests(corners: &mut [Point; 4], t: &mut Transform, x_req: &mut f64, y_req: &mut f64) {
    // calculate current width and height
    let (xmin, xmax, ymin, ymax) = min_max(corners);
    let t2 = if *x_req == 0.0 {
        Transform::scale(*y_req / (ymax - ymin), *y_req / (ymax - ymin))
    } else if *y_req == 0.0 {
        Transform::scale(*x_req / (xmax - xmin), *x_req / (xmax - xmin))
    } else {
        Transform::scale(*x_req / (xmax - xmin), *y_req / (ymax - ymin))
    };
    update_corners(corners, &t2);
    *x_req = 0.0;
    *y_req = 0.0;
    t.concat(&t2);
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// `kpse_find_file(name_of_file, kpse_pict_format, 1)` and the file's
    /// contents, as a load.
    fn read_pict(&mut self) -> Option<crate::host::OpenedFile> {
        let name = self.name_of_file.clone();
        let f = self.host.read_file(&name, FileKind::Pict);
        if T::VALUES {
            self.tracker
                .load(&name, FileKind::Pict, f.as_ref().map(|f| &f.contents));
        }
        f
    }

    /// `XeTeX`: "Implement `\XeTeXpicfile`" and "Implement
    /// `\XeTeXpdffile`".
    pub(crate) fn implement_picture(&mut self, is_pdf: bool) -> Result<(), Jump> {
        if self.mode().abs() == MMODE {
            self.report_illegal_case()
        } else {
            self.load_picture(is_pdf)
        }
    }

    /// `XeTeX`'s `load_picture`: load a picture file and handle the
    /// keywords that follow.
    fn load_picture(&mut self, is_pdf: bool) -> Result<(), Jump> {
        // scan the filename and pack into `name_of_file`
        self.scan_file_name()?;
        self.pack_file_name(self.cur_name, self.cur_area, self.cur_ext);
        let mut pdf_box_type = 0;
        let mut page = 0;
        if is_pdf {
            if self.scan_keyword(b"page")? {
                self.scan_int()?;
                page = self.cur_val;
            }
            pdf_box_type = PDFBOX_NONE;
            for (k, b) in [
                (&b"crop"[..], PDFBOX_CROP),
                (b"media", PDFBOX_MEDIA),
                (b"bleed", PDFBOX_BLEED),
                (b"trim", PDFBOX_TRIM),
                (b"art", PDFBOX_ART),
            ] {
                if self.scan_keyword(k)? {
                    pdf_box_type = b;
                    break;
                }
            }
        }
        // access the picture file and check its size
        let asked = if pdf_box_type == PDFBOX_NONE {
            PDFBOX_CROP
        } else {
            pdf_box_type
        };
        let found = self.read_pict();
        let (pic_path, bounds) = match found
            .as_ref()
            .and_then(|f| find_pic_file(&f.contents, asked, page))
        {
            Some(b) => (found.map(|f| f.name), b),
            None => (None, Rect::default()),
        };
        let mut corners = [Point::default(); 4];
        corners[0] = Point(bounds.x, bounds.y);
        corners[1] = Point(corners[0].0, bounds.y + bounds.ht);
        corners[2] = Point(bounds.x + bounds.wd, corners[1].1);
        corners[3] = Point(corners[2].0, corners[0].1);
        let mut x_size_req = 0.0;
        let mut y_size_req = 0.0;
        // look for any scaling requests for this picture
        let mut t = Transform::identity();
        loop {
            if self.scan_keyword(b"scaled")? {
                self.scan_int()?;
                if x_size_req == 0.0 && y_size_req == 0.0 {
                    let s = f64::from(self.cur_val) / 1000.0;
                    let t2 = Transform::scale(s, s);
                    update_corners(&mut corners, &t2);
                    t.concat(&t2);
                }
            } else if self.scan_keyword(b"xscaled")? {
                self.scan_int()?;
                if x_size_req == 0.0 && y_size_req == 0.0 {
                    let t2 = Transform::scale(f64::from(self.cur_val) / 1000.0, 1.0);
                    update_corners(&mut corners, &t2);
                    t.concat(&t2);
                }
            } else if self.scan_keyword(b"yscaled")? {
                self.scan_int()?;
                if x_size_req == 0.0 && y_size_req == 0.0 {
                    let t2 = Transform::scale(1.0, f64::from(self.cur_val) / 1000.0);
                    update_corners(&mut corners, &t2);
                    t.concat(&t2);
                }
            } else if self.scan_keyword(b"width")? {
                self.scan_normal_dimen()?;
                if self.cur_val <= 0 {
                    self.improper_image_size()?;
                } else {
                    x_size_req = fix2d(self.cur_val);
                }
            } else if self.scan_keyword(b"height")? {
                self.scan_normal_dimen()?;
                if self.cur_val <= 0 {
                    self.improper_image_size()?;
                } else {
                    y_size_req = fix2d(self.cur_val);
                }
            } else if self.scan_keyword(b"rotated")? {
                self.scan_decimal()?;
                if x_size_req != 0.0 || y_size_req != 0.0 {
                    size_requests(&mut corners, &mut t, &mut x_size_req, &mut y_size_req);
                }
                let t2 = Transform::rotation(fix2d(self.cur_val) * core::f64::consts::PI / 180.0);
                update_corners(&mut corners, &t2);
                let (xmin, xmax, ymin, ymax) = min_max(&corners);
                #[allow(clippy::cast_possible_truncation, reason = "C stores in a float")]
                let p = |x: f64, y: f64| Point(x as f32, y as f32);
                corners = [p(xmin, ymin), p(xmin, ymax), p(xmax, ymax), p(xmax, ymin)];
                t.concat(&t2);
            } else {
                break;
            }
        }
        if x_size_req != 0.0 || y_size_req != 0.0 {
            size_requests(&mut corners, &mut t, &mut x_size_req, &mut y_size_req);
        }
        let (xmin, xmax, ymin, ymax) = min_max(&corners);
        // (web2c makes `-xmin` `- (integer) xmin`: the corner is truncated,
        // multiplied as an integer)
        let neg = |v: f64| f64::from(c_int(v).wrapping_neg().wrapping_mul(72)) / 72.27;
        t.concat(&Transform::translation(neg(xmin), neg(ymin)));
        if let Some(path) = pic_path {
            // (`pic_page` is a quarterword: 16 bits, signed)
            #[allow(clippy::cast_possible_truncation, reason = "a quarterword")]
            let page = i32::from(page as i16);
            let node = PicNode {
                pdf: is_pdf,
                path: Arc::from(path),
                page,
                pdf_box: pdf_box_type,
                transform: [t.a, t.b, t.c, t.d, t.x, t.y].map(d2fix),
                width: d2fix(xmax - xmin),
                height: d2fix(ymax - ymin),
                depth: 0,
            };
            self.tail_append(Node::Whatsit(Box::new(Whatsit::Pic(Box::new(node)))));
        } else {
            self.print_err(b"Unable to load picture or PDF file '");
            self.print_file_name(self.cur_name, self.cur_area, self.cur_ext);
            self.print_str(b"'");
            // (`find_pic_file` never gives -43, Mac OS's "file not found")
            self.help(&[
                b"The requested image couldn't be read because",
                b"it was not a recognized image format.",
            ]);
            self.error()?;
        }
        Ok(())
    }

    /// `load_picture`'s error for a `width` or `height` not positive.
    fn improper_image_size(&mut self) -> Result<(), Jump> {
        self.print_err(b"Improper image ");
        self.print_str(b"size (");
        self.print_scaled(self.cur_val);
        self.print_str(b"pt) will be ignored");
        self.help(&[
            b"I can't scale images to zero or negative sizes,",
            b"so I'm ignoring this.",
        ]);
        self.error()
    }

    /// `\XeTeXpdfpagecount`: `scan_and_pack_name`, then
    /// `count_pdf_file_pages` (0 for a file not found or not read).
    pub(crate) fn pdf_page_count(&mut self) -> Result<i32, Jump> {
        self.scan_file_name()?;
        self.pack_file_name(self.cur_name, self.cur_area, self.cur_ext);
        Ok(self
            .read_pict()
            .and_then(|f| Doc::open(&f.contents).ok())
            .map_or(0, |doc| pp_page_count(&doc)))
    }
}

/// `find_pic_file` of the file found: its bounds, or `None` for an error.
/// A PDF's box `pdf_box` of `page` if `pdf_box` is not 0; else the
/// graphics formats it knows.
fn find_pic_file(data: &Arc<[u8]>, pdf_box: u8, page: i32) -> Option<Rect> {
    if pdf_box != 0 {
        return pdf_get_rect(data, page, pdf_box);
    }
    #[allow(clippy::cast_possible_truncation, reason = "C stores in a float")]
    let rect = |width: f64, height: f64, xdpi: f64, ydpi: f64| Rect {
        x: 0.0,
        y: 0.0,
        wd: (width * 72.27 / xdpi) as f32,
        ht: (height * 72.27 / ydpi) as f32,
    };
    if data.len() >= 2 && data[0] == 0xff && data[1] == JM_SOI {
        let (w, h, xdpi, ydpi) = jpeg_scan_file(data)?;
        return Some(rect(f64::from(w), f64::from(h), xdpi, ydpi));
    }
    if data.starts_with(b"BM") {
        let (w, h, xdpi, ydpi) = bmp_scan_file(data)?;
        return Some(rect(f64::from(w), f64::from(h), xdpi, ydpi));
    }
    if data.starts_with(&[137, 80, 78, 71]) {
        // `png_scan_file`
        let info = partex_engine::png::read_info(data).ok()?;
        // (`png_get_x_pixels_per_meter`: 0 unless the unit is the meter)
        let (x, y) = match info.phys {
            Some((x, y, 1)) => (x, y),
            _ => (0, 0),
        };
        let mut xdpi = f64::from(x) * 0.0254;
        let mut ydpi = f64::from(y) * 0.0254;
        if xdpi == 0.0 {
            xdpi = 72.0;
        }
        if ydpi == 0.0 {
            ydpi = 72.0;
        }
        // (`png_uint_32`s kept in `int`s)
        #[allow(clippy::cast_possible_wrap, reason = "C's conversion")]
        let (w, h) = (info.width as i32, info.height as i32);
        return Some(rect(f64::from(w), f64::from(h), xdpi, ydpi));
    }
    // could support other file types here (TIFF, WMF, etc?)
    None
}

const JM_SOI: u8 = 0xd8;

/// A `FILE` read by `jpegimage.c` and `numbers.c`: a position that may
/// be past the end (`fseek` allows it). `get_unsigned_byte` at the end
/// exits the program in C; here it is an error.
struct File<'a> {
    d: &'a [u8],
    pos: usize,
}

impl File<'_> {
    /// `fgetc`.
    fn getc(&mut self) -> Option<u8> {
        let c = *self.d.get(self.pos)?;
        self.pos += 1;
        Some(c)
    }

    /// `get_unsigned_byte`.
    fn byte(&mut self) -> Option<u8> {
        self.getc()
    }

    /// `get_unsigned_pair`.
    fn pair(&mut self) -> Option<u16> {
        let hi = self.byte()?;
        Some(u16::from(hi) * 0x100 + u16::from(self.byte()?))
    }

    /// `fread` of up to `n` bytes.
    fn read(&mut self, n: usize) -> &[u8] {
        let start = self.pos.min(self.d.len());
        let end = start.saturating_add(n).min(self.d.len());
        self.pos = self.pos.max(end);
        &self.d[start..end]
    }

    /// `seek_relative`.
    fn seek(&mut self, n: usize) {
        self.pos = self.pos.saturating_add(n);
    }
}

/// `JPEG_scan_file`: width, height and resolution (the JFIF density,
/// else the Exif resolution, else 72 dpi).
fn jpeg_scan_file(d: &[u8]) -> Option<(u16, u16, f64, f64)> {
    const JM_RST0: u8 = 0xd0;
    const JM_RST7: u8 = 0xd7;
    const JM_APP0: u8 = 0xe0;
    const JM_APP1: u8 = 0xe1;
    const JM_APP2: u8 = 0xe2;
    const JM_APP14: u8 = 0xee;
    let mut f = File { d, pos: 0 };
    let (mut width, mut height) = (0, 0);
    let (mut xdpi, mut ydpi) = (0.0, 0.0);
    let mut found_sofn = false;
    while !found_sofn {
        // `JPEG_get_marker`
        if f.getc() != Some(255) {
            break;
        }
        let marker = loop {
            match f.getc() {
                None => break None,
                Some(c @ 1..=254) => break Some(c),
                Some(_) => {}
            }
        };
        let Some(marker) = marker else {
            break;
        };
        if marker == JM_SOI || (JM_RST0..=JM_RST7).contains(&marker) {
            continue;
        }
        let mut length = f.pair()?.wrapping_sub(2);
        match marker {
            0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => {
                f.byte()?; // bits per component
                height = f.pair()?;
                width = f.pair()?;
                f.byte()?; // components
                found_sofn = true;
            }
            JM_APP0 => {
                if length > 5 {
                    let sig: [u8; 5] = f.read(5).try_into().ok()?;
                    length -= 5;
                    if &sig == b"JFIF\0" {
                        // `read_APP0_JFIF`
                        f.pair()?; // version
                        let units = f.byte()?;
                        let xdensity = f.pair()?;
                        let ydensity = f.pair()?;
                        let xthumb = f.byte()?;
                        let ythumb = f.byte()?;
                        #[allow(clippy::cast_possible_truncation, reason = "an unsigned short")]
                        let thumb_data_len = (3 * u32::from(xthumb) * u32::from(ythumb)) as u16;
                        f.read(usize::from(thumb_data_len));
                        (xdpi, ydpi) = match units {
                            1 => (f64::from(xdensity), f64::from(ydensity)),
                            // density is in pixels per cm
                            2 => (f64::from(xdensity) * 2.54, f64::from(ydensity) * 2.54),
                            _ => (72.0, 72.0),
                        };
                        length = length.wrapping_sub(9u16.wrapping_add(thumb_data_len));
                    } else if &sig == b"JFXX\0" {
                        // `read_APP0_JFXX`
                        f.byte()?;
                        f.seek(usize::from(length) - 1);
                        length = 0;
                    }
                }
                f.seek(usize::from(length));
            }
            JM_APP1 => {
                if length > 5 {
                    let sig: [u8; 5] = f.read(5).try_into().ok()?;
                    length -= 5;
                    if &sig == b"Exif\0" {
                        let buffer = f.read(usize::from(length)).to_vec();
                        read_app1_exif(&buffer, usize::from(length), &mut xdpi, &mut ydpi);
                        length = 0;
                    }
                }
                f.seek(usize::from(length));
            }
            JM_APP2 => {
                if length >= 14 {
                    let sig: [u8; 12] = f.read(12).try_into().ok()?;
                    length -= 12;
                    if &sig == b"ICC_PROFILE\0" {
                        // `read_APP2_ICC`
                        f.byte()?;
                        f.byte()?;
                        f.read(usize::from(length.wrapping_sub(2)));
                        length = 0;
                    }
                }
                f.seek(usize::from(length));
            }
            JM_APP14 => {
                if length > 5 {
                    let sig: [u8; 5] = f.read(5).try_into().ok()?;
                    length -= 5;
                    if &sig == b"Adobe" {
                        // `read_APP14_Adobe`
                        f.pair()?;
                        f.pair()?;
                        f.pair()?;
                        f.byte()?;
                        length = length.wrapping_sub(7);
                    }
                }
                f.seek(usize::from(length));
            }
            _ => f.seek(usize::from(length)),
        }
    }
    // If xdpi and ydpi are not yet determined, they are assumed to be 72.0
    // to avoid division by zero.
    if xdpi < 0.1 && ydpi < 0.1 {
        xdpi = 72.0;
        ydpi = 72.0;
    }
    found_sofn.then_some((width, height, xdpi, ydpi))
}

/// `read_APP1_Exif` of the `length` bytes after the signature (`buffer`,
/// fewer at the end of the file): the resolution, unless JFIF gave one.
/// (C reads past the buffer where offsets point outside it; here such
/// bytes are 0.)
#[allow(clippy::cast_possible_wrap, reason = "C's ints")]
fn read_app1_exif(buffer: &[u8], length: usize, xdpi: &mut f64, ydpi: &mut f64) {
    let at = |i: i64| -> u32 {
        usize::try_from(i)
            .ok()
            .and_then(|i| buffer.get(i))
            .map_or(0, |&b| u32::from(b))
    };
    // `read_exif_bytes`
    let read = |p: &mut i64, n: i64, big: bool| -> u32 {
        let mut v: u32 = 0;
        for k in 0..n {
            let i = if big { *p + k } else { *p + n - 1 - k };
            v = (v << 8) + at(i);
        }
        *p += n;
        v
    };
    let mut xres = 72.0;
    let mut yres = 72.0;
    let mut res_unit = 1.0;
    let mut p: i64 = 0;
    while usize::try_from(p).is_ok_and(|q| q < length) && at(p) == 0 {
        p += 1;
    }
    let tiff_header = p;
    let bigendian = match (at(p), at(p + 1)) {
        (0x4d, 0x4d) => true,  // `MM`
        (0x49, 0x49) => false, // `II`
        _ => return,
    };
    p += 2;
    if read(&mut p, 2, bigendian) != 42 {
        return;
    }
    let i = read(&mut p, 4, bigendian) as i32;
    p = tiff_header + i64::from(i);
    let mut num_fields = read(&mut p, 2, bigendian) as i32;
    let (mut value, mut num, mut den): (i32, i32, i32) = (0, 0, 0);
    while num_fields > 0 {
        num_fields -= 1;
        let tag = read(&mut p, 2, bigendian);
        let ty = read(&mut p, 2, bigendian);
        read(&mut p, 4, bigendian);
        match ty {
            // byte, undefined
            1 | 7 => {
                value = at(p) as i32;
                p += 4;
            }
            // short
            3 => {
                value = read(&mut p, 2, bigendian) as i32;
                p += 2;
            }
            // long, slong
            4 | 9 => value = read(&mut p, 4, bigendian) as i32,
            // rational, srational
            5 | 10 => {
                value = read(&mut p, 4, bigendian) as i32;
                let mut rp = tiff_header + i64::from(value);
                num = read(&mut rp, 4, bigendian) as i32;
                den = read(&mut rp, 4, bigendian) as i32;
            }
            // ascii and the rest
            _ => p += 4,
        }
        match tag {
            // x res, y res
            282 if den != 0 => xres = f64::from(num.wrapping_div(den)),
            283 if den != 0 => yres = f64::from(num.wrapping_div(den)),
            // res unit
            296 => match value {
                2 => res_unit = 1.0,
                3 => res_unit = 2.54,
                _ => {}
            },
            _ => {}
        }
    }
    // Do not overwrite if xdpi and ydpi are already determined as JFIF
    if *xdpi < 0.1 && *ydpi < 0.1 {
        *xdpi = xres * res_unit;
        *ydpi = yres * res_unit;
    }
}

/// `bmp_scan_file`: width, height and resolution.
fn bmp_scan_file(d: &[u8]) -> Option<(i32, i32, f64, f64)> {
    const DIB_FILE_HEADER_SIZE: i64 = 14;
    // (bytes C's buffer did not get read as 0)
    let at = |i: usize| d.get(i).copied().unwrap_or(0);
    #[allow(clippy::cast_possible_wrap, reason = "C's int arithmetic")]
    let ulong = |i: usize| {
        (u32::from(at(i))
            | u32::from(at(i + 1)) << 8
            | u32::from(at(i + 2)) << 16
            | u32::from(at(i + 3)) << 24) as i32
    };
    let ushort = |i: usize| i32::from(at(i)) | i32::from(at(i + 1)) << 8;
    if ulong(6) != 0 {
        // ("Not a BMP file???")
        return None;
    }
    let offset = i64::from(ulong(10));
    let hsize = i64::from(ulong(14));
    // (the info header is read whole, after the 18 bytes read)
    if d.len() < 18 || i64::try_from(d.len() - 18).ok()? < hsize - 4 || hsize < 4 {
        return None;
    }
    let p = 18;
    let (width, mut height, xdpi, ydpi, bit_count, psize);
    if hsize == 12 {
        width = ushort(p);
        height = ushort(p + 2);
        (xdpi, ydpi) = (72.0, 72.0);
        if ushort(p + 4) != 1 {
            return None;
        }
        bit_count = ushort(p + 6);
        psize = 3;
    } else if matches!(hsize, 40 | 64 | 108 | 124) {
        width = ulong(p);
        height = ulong(p + 4);
        if ushort(p + 8) != 1 {
            return None;
        }
        bit_count = ushort(p + 10);
        // (`unsigned long`s from `int`s: sign-extended)
        #[allow(
            clippy::cast_sign_loss,
            clippy::cast_precision_loss,
            reason = "C's conversions"
        )]
        let ppm = |v: i32| i64::from(v) as u64 as f64;
        xdpi = ppm(ulong(p + 20)) * 0.0254; // pixels per meter to DPI
        ydpi = ppm(ulong(p + 24)) * 0.0254;
        if height < 0 {
            height = height.wrapping_neg();
        }
        psize = 4;
    } else {
        // ("Unknown BMP header type.": C exits)
        return None;
    }
    let num_palette = if bit_count < 24 {
        if !matches!(bit_count, 1 | 4 | 8) {
            return None;
        }
        (offset - hsize - DIB_FILE_HEADER_SIZE) / psize
    } else if bit_count == 24 {
        1
    } else {
        return None;
    };
    if width == 0 || height == 0 || num_palette < 1 {
        return None;
    }
    Some((width, height, xdpi, ydpi))
}

/// `pdf_get_rect`: box `pdf_box` of page `page_num` of the PDF file
/// `data`, in TeX points (its width and height swapped by a `/Rotate` of
/// 90 or 270).
fn pdf_get_rect(data: &Arc<[u8]>, page_num: i32, pdf_box: u8) -> Option<Rect> {
    let doc = Doc::open(data).ok()?;
    let pages = pp_page_count(&doc);
    let mut page_num = page_num;
    if page_num > pages {
        page_num = pages;
    }
    if page_num < 0 {
        page_num += pages + 1;
    }
    if page_num < 1 {
        page_num = 1;
    }
    // (a page not found makes C crash)
    let pdict = pp_page(&doc, u64::try_from(page_num).ok()?)?;
    let first: &[u8] = match pdf_box {
        PDFBOX_MEDIA => b"MediaBox",
        PDFBOX_BLEED => b"BleedBox",
        PDFBOX_TRIM => b"TrimBox",
        PDFBOX_ART => b"ArtBox",
        _ => b"CropBox",
    };
    // In pplib, r can be NULL. If r == NULL, we try "CropBox", "MediaBox",
    // "BleedBox", "TrimBox", "ArtBox" in this order.
    let [lx, ly, rx, ry] = [
        first,
        b"CropBox",
        b"MediaBox",
        b"BleedBox",
        b"TrimBox",
        b"ArtBox",
    ]
    .iter()
    .find_map(|name| pp_get_box(&doc, &pdict, name))?;
    // (`ppdict_get_int`: an integer in the page's own dictionary)
    let mut rot_angle = match pdict.get(b"Rotate") {
        Some(Obj::Int(r)) => *r % 360,
        _ => 0,
    };
    if rot_angle < 0 {
        rot_angle += 360;
    }
    let (wd, ht) = if rot_angle == 90 || rot_angle == 270 {
        (
            72.27 / 72.0 * (ry - ly).abs(),
            72.27 / 72.0 * (rx - lx).abs(),
        )
    } else {
        (
            72.27 / 72.0 * (rx - lx).abs(),
            72.27 / 72.0 * (ry - ly).abs(),
        )
    };
    let my_fmin = |x: f64, y: f64| if x < y { x } else { y };
    #[allow(clippy::cast_possible_truncation, reason = "C stores in a float")]
    Some(Rect {
        x: (72.27 / 72.0 * my_fmin(lx, rx)) as f32,
        y: (72.27 / 72.0 * my_fmin(ly, ry)) as f32,
        wd: wd as f32,
        ht: ht as f32,
    })
}

/// pplib's `ppxref_pages`: the catalog's `/Pages`, by reference.
fn pp_pages(doc: &Doc) -> Option<Dict> {
    let Obj::Dict(cat) = doc.lookup(&doc.trailer, b"Root") else {
        return None;
    };
    let Some(Obj::Ref(r)) = cat.get(b"Pages") else {
        return None;
    };
    match doc.fetch(*r) {
        Obj::Dict(d) => Some(d),
        _ => None,
    }
}

/// pplib's `pppage_node`: a page tree node's `/Kids` (none for a page),
/// `/Count` and whether its `/Type` is `/Page`.
fn pp_page_node(doc: &Doc, d: &Dict) -> (Option<Vec<Obj>>, u64, bool) {
    let page = matches!(d.get(b"Type"), Some(Obj::Name(n)) if n == b"Page");
    let count = match d.get(b"Count").map(|o| doc.follow(o)) {
        Some(Obj::Int(c)) => u64::try_from(c).unwrap_or(0),
        _ => 0,
    };
    let kids = match d.get(b"Kids").map(|o| doc.follow(o)) {
        Some(Obj::Array(k)) => Some(k),
        _ => None,
    };
    (kids, count, page)
}

/// pplib's `ppdoc_page_count`.
fn pp_page_count(doc: &Doc) -> i32 {
    let Some(root) = pp_pages(doc) else {
        return 0;
    };
    #[allow(clippy::cast_possible_truncation, reason = "C's int")]
    match pp_page_node(doc, &root) {
        (None, _, page) => i32::from(page),
        (Some(_), count, _) => count as i32,
    }
}

/// pplib's `ppdoc_page`: page `index` (from 1), found by the counts of
/// the tree's nodes, from the nearer end of each node's kids.
fn pp_page(doc: &Doc, mut index: u64) -> Option<Dict> {
    let root = pp_pages(doc)?;
    let (kids, mut count, page) = pp_page_node(doc, &root);
    let Some(mut kids) = kids else {
        return (index == 1 && page).then_some(root);
    };
    if index < 1 || index > count {
        return None;
    }
    // (each descent is to a node; a loop in the tree is cut short)
    for _ in 0..64 {
        let forward = index <= count / 2;
        if !forward {
            if kids.is_empty() {
                return None;
            }
            index = count - index + 1;
        }
        let mut next = None;
        let n = kids.len();
        for i in 0..n {
            let Obj::Ref(r) = kids[if forward { i } else { n - 1 - i }] else {
                return None;
            };
            let Obj::Dict(d) = doc.fetch(r) else {
                return None;
            };
            let (k, c, page) = pp_page_node(doc, &d);
            if let Some(k) = k {
                if index <= c {
                    if !forward {
                        index = c - index + 1;
                    }
                    next = Some((k, c));
                    break;
                }
                index -= c;
                continue;
            }
            if index == 1 && page {
                return Some(d);
            }
            index -= 1;
        }
        (kids, count) = next?;
    }
    None
}

/// pplib's `ppdict_get_box`: box `name` of `d`, else of its parents.
fn pp_get_box(doc: &Doc, d: &Dict, name: &[u8]) -> Option<[f64; 4]> {
    let mut d = d.clone();
    // (a loop of parents is cut short)
    for _ in 0..64 {
        // `ppdict_get_rect`, `pparray_to_rect`
        if let Some(Obj::Array(a)) = d.get(name).map(|o| doc.follow(o))
            && a.len() == 4
            && let [Some(lx), Some(ly), Some(rx), Some(ry)] = [0, 1, 2, 3].map(|i| a[i].as_num())
        {
            return Some([lx, ly, rx, ry]);
        }
        d = match d.get(b"Parent").map(|o| doc.follow(o)) {
            Some(Obj::Dict(p)) => p,
            _ => return None,
        };
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn fixed_conversions() {
        assert_eq!(d2fix(-0.5), -32767); // truncated toward zero
        assert_eq!(d2fix(1.0), 65536);
        assert_eq!(d2fix(f64::INFINITY), i32::MIN);
        assert_eq!(d2fix(f64::NAN), i32::MIN);
        assert_eq!(c_int(-40.15), -40);
    }

    /// A JPEG of `segments` then a baseline frame header of 120×80.
    fn jpeg(segments: &[u8]) -> Vec<u8> {
        let mut d = vec![0xff, 0xd8];
        d.extend_from_slice(segments);
        d.extend_from_slice(&[0xff, 0xc0, 0, 11, 8, 0, 80, 0, 120, 1, 1, 0x11, 0]);
        d
    }

    #[test]
    fn jpeg_densities() {
        // no density: 72 dpi
        assert_eq!(jpeg_scan_file(&jpeg(&[])), Some((120, 80, 72.0, 72.0)));
        // JFIF, dots per cm
        let jfif = [
            0xff, 0xe0, 0, 16, b'J', b'F', b'I', b'F', 0, 1, 1, 2, 0, 40, 0, 50, 0, 0,
        ];
        assert_eq!(
            jpeg_scan_file(&jpeg(&jfif)),
            Some((120, 80, 40.0 * 2.54, 50.0 * 2.54))
        );
        // Exif (big-endian): x and y resolution 601/2 and 99/1 dpi, kept
        // as integer quotients
        let mut tiff = b"MM\0\x2a\0\0\0\x08\0\x02".to_vec();
        for (tag, off) in [(282u16, 38u32), (283, 46)] {
            tiff.extend_from_slice(&tag.to_be_bytes());
            tiff.extend_from_slice(&[0, 5, 0, 0, 0, 1]);
            tiff.extend_from_slice(&off.to_be_bytes());
        }
        tiff.extend_from_slice(&[0; 4]);
        for (n, d) in [(601u32, 2u32), (99, 1)] {
            tiff.extend_from_slice(&n.to_be_bytes());
            tiff.extend_from_slice(&d.to_be_bytes());
        }
        let mut exif = vec![0xff, 0xe1];
        exif.extend_from_slice(&u16::try_from(tiff.len() + 8).unwrap().to_be_bytes());
        exif.extend_from_slice(b"Exif\0\0");
        exif.extend_from_slice(&tiff);
        assert_eq!(jpeg_scan_file(&jpeg(&exif)), Some((120, 80, 300.0, 99.0)));
        // no frame header
        assert_eq!(jpeg_scan_file(&[0xff, 0xd8, 0xff, 0xd9]), None);
    }

    #[test]
    fn bmp_header() {
        let mut d = b"BM".to_vec();
        d.extend_from_slice(&[0; 8]);
        d.extend_from_slice(&54u32.to_le_bytes()); // offset
        d.extend_from_slice(&40u32.to_le_bytes()); // info header
        d.extend_from_slice(&7i32.to_le_bytes());
        d.extend_from_slice(&(-3i32).to_le_bytes()); // top-down
        d.extend_from_slice(&[1, 0, 24, 0]);
        d.extend_from_slice(&[0; 8]);
        d.extend_from_slice(&3937i32.to_le_bytes());
        d.extend_from_slice(&0i32.to_le_bytes());
        d.extend_from_slice(&[0; 8]);
        let (w, h, xdpi, ydpi) = bmp_scan_file(&d).unwrap();
        assert_eq!((w, h, ydpi), (7, 3, 0.0));
        assert!((xdpi - 3937.0 * 0.0254).abs() < 1e-12);
    }
}
