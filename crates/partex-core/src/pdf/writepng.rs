//! writepng.c: a PNG image read for `\pdfximage` (`read_png_info`) and
//! written as an image `XObject` (`write_png`): its IDAT data as they are
//! when PDF can decode them (`copy_png`), else its rows as libpng gives
//! them after pdfTeX's transformations (`partex_engine::png`), an alpha
//! channel as a soft mask, a palette as an `/Indexed` colour space.

use alloc::sync::Arc;
use alloc::vec::Vec;

use partex_engine::png::{self, Info, Transforms};

use super::image::{IMAGE_COLOR_B, IMAGE_COLOR_C, IMAGE_COLOR_I, Image};
use crate::host::Host;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;
use crate::web::*;

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
const INFO_PHYS: u32 = 0x0080;
const INFO_SRGB: u32 = 0x0800;
const INFO_ICCP: u32 = 0x1000;
const INFO_SPLT: u32 = 0x2000;

/// `PNG_FP_1`: a gamma of 1.
const FP_1: i32 = 100_000;

/// C's `round` of a double, to an `integer`.
#[allow(
    clippy::cast_possible_truncation,
    reason = "C's (integer) of a resolution"
)]
fn c_round(x: f64) -> i32 {
    x.round() as i32
}

/// `SPNG_CHUNK_IDAT`, `SPNG_CHUNK_IEND`.
const IDAT: i32 = 0x4944_4154;
const IEND: i32 = 0x4945_4E44;

/// `spng_getint` at `pos`.
fn getint(d: &[u8], pos: i64) -> Option<i32> {
    let p = usize::try_from(pos).ok()?;
    let b = d.get(p..p + 4)?;
    Some(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// The image's `png_info` again (the file as `read_png_info` read it).
fn info_of(data: &[u8]) -> Option<Info> {
    png::read_info(data).ok()
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// `read_png_info`: the image of PNG file `name` with contents `data`,
    /// and its group (`img_group_ref`: a PNG with an alpha channel in PDF
    /// 1.4 or later asks for a transparency group, one object per job,
    /// the page's if it has none yet).
    pub(crate) fn read_png_info(
        &mut self,
        name: &[u8],
        data: &alloc::sync::Arc<[u8]>,
        colorspace_ref: i32,
    ) -> Result<(Image, i32), Jump> {
        let Some(info) = info_of(data) else {
            return self.pdftex_fail(Some(name), b"libpng: internal error");
        };
        let (mut x_res, mut y_res) = (0, 0);
        if info.get_valid(INFO_PHYS) != 0 {
            // (`png_get_x_pixels_per_meter`: 0 in other units)
            let (x, y, unit) = info.phys.unwrap_or((0, 0, 0));
            let per_meter = |v: u32| if unit == 1 { f64::from(v) } else { 0.0 };
            x_res = c_round(0.0254 * per_meter(x));
            y_res = c_round(0.0254 * per_meter(y));
        }
        let color = match info.color_type {
            PALETTE => IMAGE_COLOR_C | IMAGE_COLOR_I,
            GRAY | GRAY_ALPHA => IMAGE_COLOR_B,
            RGB | RGB_ALPHA => IMAGE_COLOR_C,
            t => {
                let m = alloc::format!("unsupported type of color_type <{t}>");
                return self.pdftex_fail(Some(name), m.as_bytes());
            }
        };
        let mut group_ref = 0;
        let o = &self.pdf.out;
        if (o.fixed_major > 1 || o.fixed_minor >= 4)
            && matches!(info.color_type, GRAY_ALPHA | RGB_ALPHA)
        {
            // (a dummy group object, to make Adobe happy)
            if self.pdf.epdf.png_group == 0 {
                self.pdf.epdf.png_group = self.pdf_new_objnum()?;
            }
            if self.pdf.ship.page_group_val == 0 {
                self.pdf.ship.page_group_val = self.pdf.epdf.png_group;
            }
            group_ref = self.pdf.ship.page_group_val;
        }
        let image = Image {
            name: name.to_vec(),
            width: i32::try_from(info.width).unwrap_or(i32::MAX),
            height: i32::try_from(info.height).unwrap_or(i32::MAX),
            x_res,
            y_res,
            color,
            colorspace_ref,
            bits: i32::from(info.bit_depth),
            color_space: 0,
            data: data.clone(),
            pdf: None,
            png: true,
        };
        Ok((image, group_ref))
    }

    /// `pdf_printf("%i 0 R\n", ...)` of a colour space given.
    fn png_colorspace_ref(&mut self, r: i32) {
        self.pdf.out.objnum(r);
        self.pdf.out.print_ln(b" 0 R");
    }

    /// `write_png`, the image's dictionary begun.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn write_png(&mut self, image: &Image) -> Result<(), Jump> {
        let Some(info) = info_of(&image.data) else {
            return self.pdftex_fail(Some(&image.name), b"libpng: internal error");
        };
        let o = &mut *self.pdf.out;
        let (major, minor) = (o.fixed_major, o.fixed_minor);
        if major == 1 && minor < 5 {
            o.fixed_image_hicolor = 0;
        }
        o.print_ln(b"/Type /XObject");
        o.print_ln(b"/Subtype /Image");
        let mut copy = true;
        let mut t = Transforms::default();
        // simple transparency
        if info.get_valid(INFO_TRNS) != 0 {
            t.trns_to_alpha = true;
            copy = false;
        }
        // an alpha channel before PDF 1.4
        if major == 1 && minor < 4 && info.color_type & COLOR_MASK_ALPHA != 0 {
            t.strip_alpha = true;
            copy = false;
        }
        // 16 bits
        if info.bit_depth == 16 && o.fixed_image_hicolor == 0 {
            t.strip_16 = true;
            copy = false;
        }
        let file_gamma = (info.get_valid(INFO_GAMA) != 0).then_some(info.gamma);
        if o.fixed_image_apply_gamma != 0 {
            return self.pdf_error(
                b"ext1",
                b"\\pdfimageapplygamma is not implemented in partex yet",
            );
        }
        // (`png_read_update_info`: the rows' depth and colour type; the
        // rows themselves are read only if not copied, as pdfTeX's are)
        let (color_type, bit_depth) = png::output(&info, t);
        let o = &mut *self.pdf.out;
        o.int_entry_ln(b"Width", i64::from(info.width));
        o.int_entry_ln(b"Height", i64::from(info.height));
        o.int_entry_ln(b"BitsPerComponent", i64::from(bit_depth));
        o.print(b"/ColorSpace ");
        let others = INFO_CHRM
            | INFO_ICCP
            | INFO_SBIT
            | INFO_SRGB
            | INFO_BKGD
            | INFO_HIST
            | INFO_TRNS
            | INFO_SPLT;
        let hicolor = self.pdf.out.fixed_image_hicolor != 0;
        if copy
            && (major > 1 || minor > 1)
            && info.interlace == 0
            && matches!(color_type, GRAY | RGB)
            && file_gamma.is_none_or(|g| g == FP_1)
            && info.get_valid(others) == 0
        {
            if image.colorspace_ref != 0 {
                self.png_colorspace_ref(image.colorspace_ref);
            } else if color_type == GRAY {
                self.pdf.out.print_ln(b"/DeviceGray");
            } else {
                self.pdf.out.print_ln(b"/DeviceRGB");
            }
            self.print_str(b" (PNG copy)");
            return self.copy_png(image, &info, bit_depth);
        }
        let Some(img) = self.png_rows(image, &info, t) else {
            return self.pdftex_fail(Some(&image.name), b"libpng: internal error");
        };
        let rows = &img.rows;
        let mut needs_group = false;
        match img.color_type {
            PALETTE => {
                let palette_objnum = self.pdf_new_objnum()?;
                if image.colorspace_ref != 0 {
                    self.png_colorspace_ref(image.colorspace_ref);
                } else {
                    let n = info.palette.len();
                    let o = &mut *self.pdf.out;
                    o.print(b"[/Indexed /DeviceRGB ");
                    o.print_int(i64::try_from(n).unwrap_or(0) - 1);
                    o.print(b" ");
                    o.objnum(palette_objnum);
                    o.print_ln(b" 0 R]");
                }
                self.png_stream(rows);
                self.pdf_begin_dict(palette_objnum, 0)?;
                let bytes: Vec<u8> = info.palette.iter().flatten().copied().collect();
                self.png_stream(&bytes);
            }
            GRAY | RGB => {
                self.png_device(image.colorspace_ref, img.color_type == GRAY);
                self.png_stream(rows);
            }
            GRAY_ALPHA | RGB_ALPHA if major > 1 || minor >= 4 => {
                let gray = img.color_type == GRAY_ALPHA;
                self.png_device(image.colorspace_ref, gray);
                let smask_objnum = self.pdf_new_objnum()?;
                let o = &mut *self.pdf.out;
                o.print(b"/SMask ");
                o.objnum(smask_objnum);
                o.print_ln(b" 0 R");
                // each pixel's alpha bytes to the mask (`write_*_pixel_*`:
                // the byte's index in its row, as `j` of a row taken in
                // 16 KB pieces)
                let wide = img.bit_depth == 16 && hicolor;
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
                self.png_stream(&color);
                // the mask: 8 bits (of 16, the high byte)
                let depth = img.bit_depth;
                self.pdf_begin_dict(smask_objnum, 0)?;
                let o = &mut *self.pdf.out;
                o.print_ln(b"/Type /XObject");
                o.print_ln(b"/Subtype /Image");
                o.int_entry_ln(b"Width", i64::from(info.width));
                o.int_entry_ln(b"Height", i64::from(info.height));
                o.int_entry_ln(
                    b"BitsPerComponent",
                    if depth == 16 { 8 } else { i64::from(depth) },
                );
                o.print_ln(b"/ColorSpace /DeviceGray");
                let mask: Vec<u8> = if depth == 16 {
                    smask.iter().step_by(2).copied().collect()
                } else {
                    smask
                };
                self.png_stream(&mask);
                needs_group = true;
            }
            GRAY_ALPHA | RGB_ALPHA => {
                // (PDF before 1.4: the alpha was stripped, so this is not
                // met; pdfTeX writes the rows as gray or RGB)
                self.png_device(image.colorspace_ref, img.color_type == GRAY_ALPHA);
                self.png_stream(rows);
            }
            t => {
                let m = alloc::format!("unsupported type of color_type <{t}>");
                return self.pdftex_fail(Some(&image.name), m.as_bytes());
            }
        }
        // `write_additional_png_objects`
        if needs_group && !self.pdf.epdf.png_group_written && self.pdf.epdf.png_group > 0 {
            self.pdf.epdf.png_group_written = true;
            let g = self.pdf.epdf.png_group;
            self.pdf_begin_obj(g, 2)?;
            if self.int_par(PDF_COMPRESS_LEVEL_CODE) == 0 {
                self.pdf
                    .out
                    .print_ln(b"%PTEX Group needed for transparent pngs");
            }
            self.pdf
                .out
                .print_ln(b"<</Type/Group /S/Transparency /CS/DeviceRGB /I true>>");
            self.pdf_end_obj();
        }
        Ok(())
    }

    /// The image's rows after the transformations `t` (libpng's read),
    /// content-keyed through [`Host::cached`]: a page step run again
    /// writes its images again, and decoding a figure's PNG (inflate,
    /// the rows' filters) was most of a rebuild that ran that step, the
    /// file unchanged. The same file and transformations give the same
    /// rows, so the bytes written are the same.
    ///
    /// [`Host::cached`]: crate::host::Host::cached
    fn png_rows(&mut self, image: &Image, info: &Info, t: Transforms) -> Option<Arc<png::Image>> {
        let key = {
            use core::hash::{Hash, Hasher};
            let mut h = partex_engine::stablehash::StableHasher::new();
            b"writepng".hash(&mut h);
            h.write(&image.data);
            (t.trns_to_alpha, t.strip_alpha, t.strip_16).hash(&mut h);
            h.finish128()
        };
        if let Some(m) = self
            .host
            .cached(key)
            .and_then(|m| m.downcast::<png::Image>().ok())
        {
            return Some(m);
        }
        let img = Arc::new(png::read_image(&image.data, info, t).ok()?);
        self.host.cache(key, img.clone());
        Some(img)
    }

    /// A colour space given, or the device space.
    fn png_device(&mut self, colorspace_ref: i32, gray: bool) {
        if colorspace_ref != 0 {
            self.png_colorspace_ref(colorspace_ref);
        } else if gray {
            self.pdf.out.print_ln(b"/DeviceGray");
        } else {
            self.pdf.out.print_ln(b"/DeviceRGB");
        }
    }

    /// `pdfbeginstream`, bytes, `pdfendstream`.
    fn png_stream(&mut self, bytes: &[u8]) {
        self.pdf_begin_stream();
        for &b in bytes {
            self.pdf.out.out(b);
        }
        self.pdf_end_stream();
    }

    /// `copy_png`: the IDAT chunks' data as they are, PDF's predictor 10
    /// for their filters; the chunks walked from the signature on, twice
    /// (the length first).
    fn copy_png(&mut self, image: &Image, info: &Info, depth: u8) -> Result<(), Jump> {
        let d = &image.data[..];
        let name = image.name.clone();
        let mut pos: i64 = 8;
        let mut length: i32 = 0;
        loop {
            let (Some(len), Some(kind)) = (getint(d, pos), getint(d, pos + 4)) else {
                return self.pdftex_fail(Some(&name), b"writepng: reading chunk type failed");
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
                return self.pdftex_fail(Some(&name), b"writepng: fseek in PNG file failed");
            }
        }
        let colors = if info.color_type == RGB { 3 } else { 1 };
        let o = &mut *self.pdf.out;
        o.print(b"/Length ");
        o.print_int(i64::from(length));
        o.print(b"\n/Filter/FlateDecode\n/DecodeParms<</Colors ");
        o.print_int(colors);
        o.print(b"/Columns ");
        o.print_int(i64::from(info.width));
        o.print(b"/BitsPerComponent ");
        o.print_int(i64::from(depth));
        o.print(b"/Predictor 10>>\n>>\nstream\n");
        let mut pos: i64 = 8;
        let mut idat = 0;
        loop {
            let (Some(len), Some(kind)) = (getint(d, pos), getint(d, pos + 4)) else {
                return self.pdftex_fail(Some(&name), b"writepng: reading chunk type failed");
            };
            pos += 8;
            match kind {
                IDAT => {
                    if idat == 2 {
                        return self
                            .pdftex_fail(Some(&name), b"writepng: IDAT chunk sequence broken");
                    }
                    idat = 1;
                    let start = usize::try_from(pos).unwrap_or(usize::MAX).min(d.len());
                    let n = usize::try_from(len.max(0)).unwrap_or(0);
                    for &b in &d[start..(start + n).min(d.len())] {
                        self.pdf.out.out(b);
                    }
                    pos += i64::from(len.max(0)) + 4;
                }
                IEND => {
                    self.pdf_end_stream();
                    return Ok(());
                }
                _ => {
                    if idat == 1 {
                        idat = 2;
                    }
                    pos += i64::from(len) + 4;
                    if pos < 0 {
                        return self
                            .pdftex_fail(Some(&name), b"writepng: fseek in PNG file failed");
                    }
                }
            }
        }
    }
}
