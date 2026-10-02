//! pdfTeX's images (`\pdfximage`, writeimg.c): reading an image's size,
//! scaling it (`scale_image`), placing it (`out_image`) and writing its
//! `XObject`. JPEG is read natively (writejpg.c: the file goes into the
//! PDF as it is); PNG, JBIG2 and PDF inclusion are not written yet.

use alloc::sync::Arc;
use alloc::vec::Vec;

use super::objtab::{Aux, Id, OBJ_TYPE_XIMAGE, XImage};
use super::out::TEN_POW;
use super::{ONE_HUNDRED_BP, ONE_HUNDRED_INCH};
use crate::host::Host;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;
use crate::web::*;
use partex_engine::scaled::Scaled;

/// `one_inch`.
const ONE_INCH: Scaled = (ONE_HUNDRED_INCH + 50) / 100;

/// image.h's `IMAGE_COLOR_*`: the `/ProcSet` an image needs.
pub(crate) const IMAGE_COLOR_B: i32 = 1;
pub(crate) const IMAGE_COLOR_C: i32 = 2;
pub(crate) const IMAGE_COLOR_I: i32 = 4;

const JPG_GRAY: u8 = 1;
const JPG_RGB: u8 = 3;
const JPG_CMYK: u8 = 4;

/// A read image (`image_entry` with its JPEG part).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Image {
    /// The path as found (`img_name`), printed when written.
    pub name: Vec<u8>,
    pub width: i32,
    pub height: i32,
    pub x_res: i32,
    pub y_res: i32,
    /// `img_color`: an `IMAGE_COLOR_*` bit.
    pub color: i32,
    pub colorspace_ref: i32,
    pub bits: i32,
    color_space: u8,
    data: Arc<[u8]>,
}

partex_engine::persist_struct!(Image {
    name,
    width,
    height,
    x_res,
    y_res,
    color,
    colorspace_ref,
    bits,
    color_space,
    data
});

pub(crate) use partex_engine::scaled::ext_xn_over_d;

/// writejpg.c's `read_jpg_info`, or the message of `pdftex_fail`.
fn read_jpg_info(
    name: Vec<u8>,
    data: Arc<[u8]>,
    colorspace_ref: i32,
    pdf_12: bool,
) -> Result<Image, &'static [u8]> {
    let d = &data[..];
    let two = |i: usize| -> u16 {
        u16::from(*d.get(i).unwrap_or(&0)) << 8 | u16::from(*d.get(i + 1).unwrap_or(&0))
    };
    if two(0) != 0xFFD8 {
        return Err(b"reading JPEG image failed (no JPEG header found)");
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
                    #[expect(clippy::cast_possible_truncation, reason = "C's integer *= 2.54")]
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
        0xFFE1 => return Err(b"Exif JPEG images are not implemented in partex yet"),
        _ => {}
    }
    let mut i = 0;
    loop {
        if i >= d.len() {
            return Err(b"reading JPEG image failed (premature file end)");
        }
        if d[i] != 0xFF {
            return Err(b"reading JPEG image failed (no marker found)");
        }
        let m = d.get(i + 1).copied().unwrap_or(0);
        i += 2;
        match m {
            0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                return Err(b"unsupported type of compression");
            }
            0xC0..=0xC3 => {
                if m == 0xC2 && pdf_12 {
                    return Err(b"cannot use progressive DCT with PDF-1.2");
                }
                let bits = i32::from(d.get(i + 2).copied().unwrap_or(0));
                let height = i32::from(two(i + 3));
                let width = i32::from(two(i + 5));
                let color_space = d.get(i + 7).copied().unwrap_or(0);
                let color = match color_space {
                    JPG_GRAY => IMAGE_COLOR_B,
                    JPG_RGB | JPG_CMYK => IMAGE_COLOR_C,
                    _ => return Err(b"Unsupported color space"),
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
                });
            }
            // markers without parameters
            0xD8 | 0xD9 | 0x01 | 0xD0..=0xD7 => {}
            _ => i += usize::from(two(i)),
        }
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// pdfTeX §1552–§1553: `\pdfximage` (`scan_image`).
    pub(crate) fn implement_pdfximage(&mut self) -> Result<(), Jump> {
        self.check_pdfoutput(b"\\pdfximage", true)?;
        self.check_pdfversion()?;
        self.pdf.ximage_count += 1;
        let k = self.pdf_create_obj(OBJ_TYPE_XIMAGE, Id::Num(self.pdf.ximage_count))?;
        let dims = self.scan_alt_rule()?;
        let attr = if self.scan_keyword(b"attr")? {
            Some(self.scan_pdf_ext_toks()?)
        } else {
            None
        };
        if self.scan_keyword(b"named")? {
            self.scan_pdf_ext_toks()?;
        } else if self.scan_keyword(b"page")? {
            self.scan_int()?;
        }
        let colorspace = if self.scan_keyword(b"colorspace")? {
            self.scan_int()?;
            self.cur_val
        } else {
            0
        };
        self.scan_pdf_box_spec()?;
        let t = self.scan_pdf_ext_toks()?;
        let s = self.tokens_string(&t);
        let s = crate::pdfconv::c_string(&s).to_vec();
        let image = self.read_image(&s, colorspace)?;
        self.pdf.objs.get_mut(k).aux = Aux::XImage(alloc::boxed::Box::new(XImage {
            width: dims.width,
            height: dims.height,
            depth: dims.depth,
            attr,
            image: Some(image.clone()),
        }));
        self.scale_image(k, &image)?;
        self.set_pdf_last(crate::pdf::PdfLast::XImage, k);
        self.set_pdf_last(crate::pdf::PdfLast::XImagePages, 1);
        self.set_pdf_last(crate::pdf::PdfLast::XImageColordepth, image.bits);
        Ok(())
    }

    /// `scan_pdf_box_spec`.
    fn scan_pdf_box_spec(&mut self) -> Result<(), Jump> {
        for k in [
            &b"mediabox"[..],
            b"cropbox",
            b"bleedbox",
            b"trimbox",
            b"artbox",
        ] {
            if self.scan_keyword(k)? {
                break;
            }
        }
        Ok(())
    }

    /// writeimg.c's `read_image`.
    fn read_image(&mut self, s: &[u8], colorspace: i32) -> Result<Arc<Image>, Jump> {
        // (a load: a name the job stores reads its store, DESIGN 3.7)
        let found = self.read_source(s);
        let Some(f) = found else {
            let mut m = b"cannot find image file ".to_vec();
            m.extend_from_slice(s);
            return self.pdftex_fail(None, &m);
        };
        let d = &f.contents;
        let jpg = d.starts_with(b"\xFF\xD8");
        let other =
            d.starts_with(b"\x89PNG") || d.starts_with(b"\x97JB2") || d.starts_with(b"%PDF");
        let ext = s
            .rsplit(|&c| c == b'.')
            .next()
            .map(<[u8]>::to_ascii_lowercase);
        if other || (!jpg && ext.as_deref() != Some(b"jpg") && ext.as_deref() != Some(b"jpeg")) {
            return self.pdf_error(
                b"ext1",
                b"PNG, JBIG2 and PDF images are not implemented in partex yet",
            );
        }
        let pdf_12 = self.pdf.out.fixed_major == 1 && self.pdf.out.fixed_minor <= 2;
        match read_jpg_info(f.name.clone(), f.contents.clone(), colorspace, pdf_12) {
            Ok(i) => Ok(Arc::new(i)),
            Err(m) => self.pdftex_fail(Some(&f.name), m),
        }
    }

    /// pdfTeX §1552: `scale_image`.
    fn scale_image(&mut self, n: i32, image: &Image) -> Result<(), Jump> {
        let (x, y) = (image.width, image.height);
        let (mut xr, mut yr) = (image.x_res, image.y_res);
        if xr > 65535 || yr > 65535 {
            (xr, yr) = (0, 0);
            self.pdf_warning(b"ext1", b"too large image resolution ignored", true, true);
        }
        if x <= 0 || y <= 0 || xr < 0 || yr < 0 {
            return self.pdf_error(b"ext1", b"invalid image dimensions");
        }
        if (xr != 0 || yr != 0) && (x / ONE_INCH >= xr || y / ONE_INCH >= yr) {
            (xr, yr) = (0, 0);
            self.pdf_warning(b"ext1", b"too small image resolution ignored", true, true);
        }
        let default_res = self.int_par(PDF_IMAGE_RESOLUTION_CODE).clamp(0, 65535);
        if default_res > 0 && (xr == 0 || yr == 0) {
            (xr, yr) = (default_res, default_res);
        }
        let Aux::XImage(xi) = &mut self.pdf.objs.get_mut(n).aux else {
            return Ok(());
        };
        let running = |v: i32| v == super::ext::RUNNING;
        let (mut w, mut h) = (0, 0);
        if running(xi.width) && running(xi.height) {
            if xr > 0 && yr > 0 {
                w = ext_xn_over_d(ONE_HUNDRED_INCH, x, 100 * xr);
                h = ext_xn_over_d(ONE_HUNDRED_INCH, y, 100 * yr);
            } else {
                w = ext_xn_over_d(ONE_HUNDRED_INCH, x, 7200);
                h = ext_xn_over_d(ONE_HUNDRED_INCH, y, 7200);
            }
        }
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

    fn ximage(&self, n: i32) -> Option<(&XImage, Arc<Image>)> {
        match &self.pdf.objs.get(n).aux {
            Aux::XImage(x) => Some((x, x.image.clone()?)),
            _ => None,
        }
    }

    /// pdfTeX §1629: `out_image`.
    pub(crate) fn emit_image(
        &mut self,
        objnum: i32,
        dims: partex_engine::node::Dims,
    ) -> Result<(), Jump> {
        let Some((_, image)) = self.ximage(objnum) else {
            return Ok(());
        };
        self.pdf_end_text();
        self.pdf.out.print_ln(b"q");
        if !self.pdf.ship.ximage_list.contains(&objnum) {
            self.pdf.ship.ximage_list.push(objnum);
        }
        let _ = image;
        let a = ext_xn_over_d(dims.width, TEN_POW[6], ONE_HUNDRED_BP);
        self.pdf.out.print_real(a, 4);
        self.pdf.out.print(b" 0 0 ");
        let b = ext_xn_over_d(dims.height + dims.depth, TEN_POW[6], ONE_HUNDRED_BP);
        self.pdf.out.print_real(b, 4);
        self.pdf.out.out(b' ');
        let s = &self.pdf.ship;
        let (x, y) = (s.cur_h - s.origin_h, s.origin_v - s.cur_v);
        self.pdf_print_bp(x)?;
        self.pdf.out.out(b' ');
        self.pdf_print_bp(y)?;
        self.pdf.out.print_ln(b" cm");
        self.pdf.out.print(b"/Im");
        let n = self.pdf.objs.get(objnum).info.num();
        self.pdf.out.print_int(i64::from(n));
        self.pdf_print_resname_prefix();
        self.pdf.out.print_ln(b" Do");
        self.pdf.out.print_ln(b"Q");
        Ok(())
    }

    /// `update_image_procset`.
    pub(crate) fn image_color(&self, n: i32) -> i32 {
        self.ximage(n).map_or(0, |(_, i)| i.color)
    }

    /// pdfTeX §1628: `pdf_write_image`.
    pub(crate) fn pdf_write_image(&mut self, n: i32) -> Result<(), Jump> {
        use super::val::{SHIPPING, SHIPPING_READS};
        self.writer_scope(SHIPPING_READS, SHIPPING, |t| t.pdf_write_image_now(n))
    }

    fn pdf_write_image_now(&mut self, n: i32) -> Result<(), Jump> {
        let Some((xi, image)) = self.ximage(n) else {
            return Ok(());
        };
        let attr = xi.attr.clone();
        self.pdf_begin_dict(n, 0)?;
        if let Some(a) = &attr {
            self.pdf_print_toks_ln(a);
        }
        if self.pdf.out.fixed_draftmode == 0 {
            self.print_str(b" <");
            self.print_str(&image.name);
            let o = &mut *self.pdf.out;
            o.print_ln(b"/Type /XObject");
            o.print_ln(b"/Subtype /Image");
            o.int_entry_ln(b"Width", i64::from(image.width));
            o.int_entry_ln(b"Height", i64::from(image.height));
            o.int_entry_ln(b"BitsPerComponent", i64::from(image.bits));
            o.int_entry_ln(b"Length", i64::try_from(image.data.len()).unwrap_or(0));
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
                o.objnum(image.colorspace_ref);
                o.print_ln(b" 0 R");
            }
            o.print_ln(b"/Filter /DCTDecode");
            o.print_ln(b">>");
            o.print_ln(b"stream");
            for &b in image.data.iter() {
                o.out(b);
            }
            self.pdf_end_stream();
            self.print_str(b">");
        }
        if let Aux::XImage(xi) = &mut self.pdf.objs.get_mut(n).aux {
            xi.attr = None;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_jfif_header() {
        let mut d =
            b"\xFF\xD8\xFF\xE0\x00\x10JFIF\x00\x01\x01\x01\x00\x48\x00\x48\x00\x00".to_vec();
        d.extend_from_slice(b"\xFF\xC0\x00\x11\x08\x00\x20\x00\x40\x03");
        let i = read_jpg_info(Vec::new(), Arc::from(d), 0, false).unwrap();
        assert_eq!(
            (i.width, i.height, i.bits, i.x_res, i.color),
            (64, 32, 8, 72, IMAGE_COLOR_C)
        );
        assert_eq!(ext_xn_over_d(7, 3, 2), 11);
        assert_eq!(ext_xn_over_d(-7, 3, 2), -11);
    }
}
