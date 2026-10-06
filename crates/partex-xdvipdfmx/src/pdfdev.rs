//! pdfdev.c, pdfdev.h: the output device (text, rules, images, units).
//!
//! C's single `static pdf_dev pdev` is `self.dev.pdev`; the static
//! functions that take `pdf_dev *p` are `Dpx` methods without it. The
//! `pdf_sprint_*` writers append to a [`Buf`] and return the bytes
//! written (C's `char *buf` + length). `p_itoa`/`p_dtoa` are in
//! [`crate::fmt`].

use crate::fmt::{Buf, p_dtoa, round_acc};
use crate::fontmap::FontmapRec;
use crate::prelude::*;

/// `spt_t`: a length in DVI units (scaled points).
pub type Spt = i32;

/// `pdf_tmatrix`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PdfTmatrix {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

/// `pdf_rect`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PdfRect {
    pub llx: f64,
    pub lly: f64,
    pub urx: f64,
    pub ury: f64,
}

/// `pdf_coord`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PdfCoord {
    pub x: f64,
    pub y: f64,
}

/// `transform_info`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TransformInfo {
    pub width: f64,
    pub height: f64,
    pub depth: f64,
    /// Transform matrix.
    pub matrix: PdfTmatrix,
    /// User bbox.
    pub bbox: PdfRect,
    pub flags: i32,
}

pub const INFO_HAS_USER_BBOX: i32 = 1 << 0;
pub const INFO_HAS_WIDTH: i32 = 1 << 1;
pub const INFO_HAS_HEIGHT: i32 = 1 << 2;
pub const INFO_DO_CLIP: i32 = 1 << 3;
pub const INFO_DO_HIDE: i32 = 1 << 4;

/// `PDF_DEV_PARAM_AUTOROTATE`.
pub const PDF_DEV_PARAM_AUTOROTATE: i32 = 1;
/// `PDF_DEV_PARAM_COLORMODE`.
pub const PDF_DEV_PARAM_COLORMODE: i32 = 2;

/// Motion state: not within BT/ET nor in a string.
pub const GRAPHICS_MODE: i32 = 1;
/// Motion state: in BT/ET, not in a string.
pub const TEXT_MODE: i32 = 2;
/// Motion state: in a string.
pub const STRING_MODE: i32 = 3;

pub const TEXT_WMODE_HH: i32 = 0;
pub const TEXT_WMODE_HV: i32 = 1;
pub const TEXT_WMODE_VH: i32 = 4;
pub const TEXT_WMODE_VV: i32 = 5;
pub const TEXT_WMODE_HD: i32 = 3;
pub const TEXT_WMODE_VD: i32 = 7;

pub const PDF_FONTTYPE_SIMPLE: i32 = 1;
pub const PDF_FONTTYPE_BITMAP: i32 = 2;
pub const PDF_FONTTYPE_COMPOSITE: i32 = 3;

/// `TEX_ONE_HUNDRED_BP`.
pub const TEX_ONE_HUNDRED_BP: i32 = 6578176;
/// `FORMAT_BUF_SIZE`.
pub const FORMAT_BUF_SIZE: usize = 4096;
/// `DEV_PRECISION_MAX`.
pub const DEV_PRECISION_MAX: i32 = 8;
/// `PDF_LINE_THICKNESS_MAX`.
pub const PDF_LINE_THICKNESS_MAX: f64 = 5.0;

/// `ten_pow`.
pub const TEN_POW: [u32; 10] = [
    1, 10, 100, 1000, 10000, 100000, 1000000, 10000000, 100000000, 1000000000,
];
/// `ten_pow_inv`.
pub const TEN_POW_INV: [f64; 10] = [
    1.0,
    0.1,
    0.01,
    0.001,
    0.0001,
    0.00001,
    0.000001,
    0.0000001,
    0.00000001,
    0.000000001,
];

/// `struct dev_param`.
#[derive(Clone, Copy, Debug, Default)]
pub struct DevParam {
    pub autorotate: i32,
    /// 0: ignore colors.
    pub colormode: i32,
}

/// `text_state.matrix`.
#[derive(Clone, Copy, Debug, Default)]
pub struct TextMatrix {
    pub slant: f64,
    pub extend: f64,
    /// `TEXT_WMODE_XX`.
    pub rotate: i32,
}

/// The linear part `[a b c d]` of the text matrix `dev_set_text_matrix`
/// sets for a font's `slant` and `extend` in direction `rotate`
/// (`TEXT_WMODE_XX`; any other: all zero, as C leaves it).
#[must_use]
pub fn text_matrix(slant: f64, extend: f64, rotate: i32) -> [f64; 4] {
    match rotate {
        TEXT_WMODE_VH => [slant, 1.0, -extend, 0.0],
        TEXT_WMODE_HV => [0.0, -extend, 1.0, -slant],
        TEXT_WMODE_HH => [extend, 0.0, slant, 1.0],
        TEXT_WMODE_VV => [1.0, -slant, 0.0, extend],
        TEXT_WMODE_HD => [0.0, extend, -1.0, slant],
        TEXT_WMODE_VD => [-1.0, slant, 0.0, -extend],
        _ => [0.0; 4],
    }
}

/// `struct text_state`.
#[derive(Clone, Copy, Debug, Default)]
pub struct TextState {
    /// Index into `PdfDev::fonts` (-1: none).
    pub font_id: i32,
    pub offset: Spt,
    pub ref_x: Spt,
    pub ref_y: Spt,
    pub raise: Spt,
    pub leading: Spt,
    pub matrix: TextMatrix,
    pub bold_param: f64,
    pub dir_mode: i32,
    pub force_reset: bool,
    pub is_mb: bool,
}

/// `struct dev_font`.
///
/// C's `used_chars` (a pointer into the pdf_font's usedchars) is not a
/// field: mark glyphs through `font_id` (`pdf_get_font_usedchars`
/// semantics, the descendant's for a Type0 font).
#[derive(Clone, Debug, Default)]
pub struct DevFont {
    /// The pdf_font's used-glyph table (C's `used_chars`), once the font
    /// is set.
    pub used_chars: Option<crate::pdffont::UsedChars>,
    /// Resource name ("F12"; C's `char short_name[16]`).
    pub short_name: Vec<u8>,
    pub used_on_this_page: bool,
    pub tex_name: Vec<u8>,
    pub sptsize: Spt,
    /// The pdf_font id.
    pub font_id: i32,
    /// The encoding (CMap) id.
    pub enc_id: i32,
    pub resource: Option<Obj>,
    /// `PDF_FONTTYPE_*`.
    pub format: i32,
    /// Non-zero for vertical.
    pub wmode: i32,
    pub extend: f64,
    pub slant: f64,
    pub bold: f64,
}

/// `struct dev_unit`.
#[derive(Clone, Copy, Debug, Default)]
pub struct DevUnit {
    /// DVI unit to bp multiplier.
    pub dvi2pts: f64,
    /// Shortest resolvable distance in the output (DVI units).
    pub min_bp_val: i32,
    /// Decimal digits kept.
    pub precision: i32,
}

/// `struct pdf_dev`.
#[derive(Clone, Debug, Default)]
pub struct PdfDev {
    pub motion_state: i32,
    pub param: DevParam,
    pub unit: DevUnit,
    pub text_state: TextState,
    /// `fonts` / `num_dev_fonts` / `max_dev_fonts`.
    pub fonts: Vec<DevFont>,
    /// `format_buffer` (scratch).
    pub format_buffer: Buf,
}

/// pdfdev.c's statics.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// `pdev`.
    pub pdev: PdfDev,
}

impl TransformInfo {
    /// `transform_info_clear`.
    pub fn transform_info_clear(&mut self) {
        *self = TransformInfo {
            matrix: PdfTmatrix {
                a: 1.0,
                d: 1.0,
                ..PdfTmatrix::default()
            },
            ..TransformInfo::default()
        };
    }
}

/// `pdf_setmatrix`.
pub fn pdf_setmatrix(m: &mut PdfTmatrix, a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) {
    *m = PdfTmatrix { a, b, c, d, e, f };
}

/// `ANGLE_CHANGES(m1, m2)`.
#[must_use]
pub fn angle_changes(m1: i32, m2: i32) -> bool {
    (m1 - m2).abs() % 5 != 0
}

/// `ROTATE_TEXT(m)`.
#[must_use]
pub fn rotate_text(m: i32) -> bool {
    m != TEXT_WMODE_HH && m != TEXT_WMODE_VV
}

impl Dpx {
    fn dev_out(&mut self, s: &[u8]) -> Result<()> {
        self.pdf_doc_add_page_content(s)?;
        Ok(())
    }

    fn bpt2spt(&self, b: f64) -> Spt {
        libm::round(b / self.dev.pdev.unit.dvi2pts) as Spt
    }

    fn dev_sprint_bp(&self, buf: &mut Buf, value: Spt, error: Option<&mut Spt>) {
        let prec = self.dev.pdev.unit.precision;
        let value_in_bp = f64::from(value) * self.dev.pdev.unit.dvi2pts;
        if let Some(e) = error {
            let error_in_bp = value_in_bp - round_acc(value_in_bp, TEN_POW_INV[prec as usize]);
            *e = self.bpt2spt(error_in_bp);
        }
        p_dtoa(buf, value_in_bp, prec as usize);
    }

    /// `pdf_sprint_matrix`.
    pub fn pdf_sprint_matrix(&self, buf: &mut Buf, m: &PdfTmatrix) {
        let prec2 = (self.dev.pdev.unit.precision + 2).min(DEV_PRECISION_MAX) as usize;
        let prec0 = self.dev.pdev.unit.precision.max(2) as usize;
        p_dtoa(buf, m.a, prec2);
        buf.push(b' ');
        p_dtoa(buf, m.b, prec2);
        buf.push(b' ');
        p_dtoa(buf, m.c, prec2);
        buf.push(b' ');
        p_dtoa(buf, m.d, prec2);
        buf.push(b' ');
        p_dtoa(buf, m.e, prec0);
        buf.push(b' ');
        p_dtoa(buf, m.f, prec0);
    }

    /// `pdf_sprint_rect`.
    pub fn pdf_sprint_rect(&self, buf: &mut Buf, r: &PdfRect) {
        let p = self.dev.pdev.unit.precision as usize;
        p_dtoa(buf, r.llx, p);
        buf.push(b' ');
        p_dtoa(buf, r.lly, p);
        buf.push(b' ');
        p_dtoa(buf, r.urx, p);
        buf.push(b' ');
        p_dtoa(buf, r.ury, p);
    }

    /// `pdf_sprint_coord`.
    pub fn pdf_sprint_coord(&self, buf: &mut Buf, c: &PdfCoord) {
        let p = self.dev.pdev.unit.precision as usize;
        p_dtoa(buf, c.x, p);
        buf.push(b' ');
        p_dtoa(buf, c.y, p);
    }

    /// `pdf_sprint_length`.
    pub fn pdf_sprint_length(&self, buf: &mut Buf, v: f64) {
        p_dtoa(buf, v, self.dev.pdev.unit.precision as usize);
    }

    /// `pdf_sprint_number`.
    pub fn pdf_sprint_number(&self, buf: &mut Buf, v: f64) {
        p_dtoa(buf, v, DEV_PRECISION_MAX as usize);
    }

    fn dev_set_text_matrix(
        &mut self,
        xpos: Spt,
        ypos: Spt,
        slant: f64,
        extend: f64,
        rotate: i32,
    ) -> Result<()> {
        let [a, b, c, d] = text_matrix(slant, extend, rotate);
        let mut tm = PdfTmatrix {
            a,
            b,
            c,
            d,
            ..PdfTmatrix::default()
        };
        tm.e = f64::from(xpos) * self.dev.pdev.unit.dvi2pts;
        tm.f = f64::from(ypos) * self.dev.pdev.unit.dvi2pts;
        let mut b = Buf::new();
        b.push(b' ');
        self.pdf_sprint_matrix(&mut b, &tm);
        b.extend(b" Tm");
        self.dev_out(&b.0)?;
        let ts = &mut self.dev.pdev.text_state;
        ts.ref_x = xpos;
        ts.ref_y = ypos;
        ts.matrix.slant = slant;
        ts.matrix.extend = extend;
        ts.matrix.rotate = rotate;
        Ok(())
    }

    fn reset_text_state(&mut self) -> Result<()> {
        self.dev_out(b" BT")?;
        let ts = self.dev.pdev.text_state;
        if ts.force_reset
            || ts.matrix.slant != 0.0
            || ts.matrix.extend != 1.0
            || rotate_text(ts.matrix.rotate)
        {
            self.dev_set_text_matrix(0, 0, ts.matrix.slant, ts.matrix.extend, ts.matrix.rotate)?;
        }
        let ts = &mut self.dev.pdev.text_state;
        ts.ref_x = 0;
        ts.ref_y = 0;
        ts.offset = 0;
        ts.force_reset = false;
        Ok(())
    }

    fn pdf_dev_text_mode(&mut self) -> Result<()> {
        match self.dev.pdev.motion_state {
            TEXT_MODE => {}
            STRING_MODE => {
                let s: &[u8] = if self.dev.pdev.text_state.is_mb {
                    b">]TJ"
                } else {
                    b")]TJ"
                };
                self.dev_out(s)?;
            }
            GRAPHICS_MODE => self.reset_text_state()?,
            _ => {}
        }
        self.dev.pdev.motion_state = TEXT_MODE;
        self.dev.pdev.text_state.offset = 0;
        Ok(())
    }

    fn pdf_dev_graphics_mode(&mut self) -> Result<()> {
        match self.dev.pdev.motion_state {
            GRAPHICS_MODE => {}
            STRING_MODE | TEXT_MODE => {
                if self.dev.pdev.motion_state == STRING_MODE {
                    let s: &[u8] = if self.dev.pdev.text_state.is_mb {
                        b">]TJ"
                    } else {
                        b")]TJ"
                    };
                    self.dev_out(s)?;
                }
                if self.dev.pdev.text_state.bold_param != 0.0 {
                    self.dev_out(b" 0 Tr")?;
                    self.dev.pdev.text_state.bold_param = 0.0;
                }
                self.dev_out(b" ET")?;
                self.dev.pdev.text_state.force_reset = false;
                self.dev.pdev.text_state.font_id = -1;
            }
            _ => {}
        }
        self.dev.pdev.motion_state = GRAPHICS_MODE;
        Ok(())
    }

    fn start_string(
        &mut self,
        xpos: Spt,
        ypos: Spt,
        slant: f64,
        extend: f64,
        rotate: i32,
    ) -> Result<()> {
        let delx = xpos - self.dev.pdev.text_state.ref_x;
        let dely = ypos - self.dev.pdev.text_state.ref_y;
        let (mut error_delx, mut error_dely) = (0, 0);
        let mut b = Buf::new();
        match rotate {
            TEXT_WMODE_VH => {
                let desired_delx = dely;
                let desired_dely = (-(f64::from(delx) - f64::from(dely) * slant) / extend) as Spt;
                b.push(b' ');
                self.dev_sprint_bp(&mut b, desired_delx, Some(&mut error_dely));
                b.push(b' ');
                self.dev_sprint_bp(&mut b, desired_dely, Some(&mut error_delx));
                error_delx = -error_delx;
            }
            TEXT_WMODE_HV => {
                let desired_delx = (-(f64::from(dely) + f64::from(delx) * slant) / extend) as Spt;
                let desired_dely = delx;
                b.push(b' ');
                self.dev_sprint_bp(&mut b, desired_delx, Some(&mut error_dely));
                b.push(b' ');
                self.dev_sprint_bp(&mut b, desired_dely, Some(&mut error_delx));
                error_dely = -error_dely;
            }
            TEXT_WMODE_HH => {
                let desired_delx = ((f64::from(delx) - f64::from(dely) * slant) / extend) as Spt;
                let desired_dely = dely;
                b.push(b' ');
                self.dev_sprint_bp(&mut b, desired_delx, Some(&mut error_delx));
                b.push(b' ');
                self.dev_sprint_bp(&mut b, desired_dely, Some(&mut error_dely));
            }
            TEXT_WMODE_VV => {
                let desired_delx = delx;
                let desired_dely = ((f64::from(dely) + f64::from(delx) * slant) / extend) as Spt;
                b.push(b' ');
                self.dev_sprint_bp(&mut b, desired_delx, Some(&mut error_delx));
                b.push(b' ');
                self.dev_sprint_bp(&mut b, desired_dely, Some(&mut error_dely));
            }
            TEXT_WMODE_HD => {
                let desired_delx =
                    -((-(f64::from(dely) + f64::from(delx) * slant) / extend) as Spt);
                let desired_dely = -delx;
                b.push(b' ');
                self.dev_sprint_bp(&mut b, desired_delx, Some(&mut error_dely));
                b.push(b' ');
                self.dev_sprint_bp(&mut b, desired_dely, Some(&mut error_delx));
                error_delx = -error_delx;
                error_dely = -error_dely;
            }
            TEXT_WMODE_VD => {
                let desired_delx = -delx;
                let desired_dely = -(((f64::from(dely) + f64::from(delx) * slant) / extend) as Spt);
                b.push(b' ');
                self.dev_sprint_bp(&mut b, desired_delx, Some(&mut error_delx));
                b.push(b' ');
                self.dev_sprint_bp(&mut b, desired_dely, Some(&mut error_dely));
                error_delx = -error_delx;
                error_dely = -error_dely;
            }
            _ => {}
        }
        self.dev_out(&b.0)?;
        let s: &[u8] = if self.dev.pdev.text_state.is_mb {
            b" Td[<"
        } else {
            b" Td[("
        };
        self.dev_out(s)?;
        let ts = &mut self.dev.pdev.text_state;
        ts.ref_x = xpos - error_delx;
        ts.ref_y = ypos - error_dely;
        ts.offset = 0;
        Ok(())
    }

    fn pdf_dev_string_mode(
        &mut self,
        xpos: Spt,
        ypos: Spt,
        slant: f64,
        extend: f64,
        rotate: i32,
    ) -> Result<()> {
        match self.dev.pdev.motion_state {
            STRING_MODE => {}
            GRAPHICS_MODE | TEXT_MODE => {
                if self.dev.pdev.motion_state == GRAPHICS_MODE {
                    self.reset_text_state()?;
                }
                if self.dev.pdev.text_state.force_reset {
                    self.dev_set_text_matrix(xpos, ypos, slant, extend, rotate)?;
                    let s: &[u8] = if self.dev.pdev.text_state.is_mb {
                        b"[<"
                    } else {
                        b"[("
                    };
                    self.dev_out(s)?;
                    self.dev.pdev.text_state.force_reset = false;
                } else {
                    self.start_string(xpos, ypos, slant, extend, rotate)?;
                }
            }
            _ => {}
        }
        self.dev.pdev.motion_state = STRING_MODE;
        Ok(())
    }

    fn pdf_dev_set_font(&mut self, font_id: i32) -> Result<()> {
        self.pdf_dev_text_mode()?;
        let fi = font_id as usize;
        let (format, wmode, slant, extend) = {
            let f = &self.dev.pdev.fonts[fi];
            (f.format, f.wmode, f.slant, f.extend)
        };
        self.dev.pdev.text_state.is_mb = format == PDF_FONTTYPE_COMPOSITE;
        let vert_font = i32::from(wmode != 0);
        let vert_dir = if self.dev.pdev.param.autorotate != 0 {
            self.dev.pdev.text_state.dir_mode
        } else {
            vert_font
        };
        let text_rotate = (vert_font << 2) | vert_dir;
        let ts = &mut self.dev.pdev.text_state;
        if slant != ts.matrix.slant
            || extend != ts.matrix.extend
            || angle_changes(text_rotate, ts.matrix.rotate)
        {
            ts.force_reset = true;
        }
        ts.matrix.slant = slant;
        ts.matrix.extend = extend;
        ts.matrix.rotate = text_rotate;
        if self.dev.pdev.fonts[fi].resource.is_none() {
            let id = self.dev.pdev.fonts[fi].font_id;
            let r = self.pdf_get_font_reference(id)?;
            self.dev.pdev.fonts[fi].resource = Some(r);
            self.dev.pdev.fonts[fi].used_chars = self.pdf_get_font_usedchars(id)?;
        }
        if !self.dev.pdev.fonts[fi].used_on_this_page {
            let r = self.dev.pdev.fonts[fi].resource.expect("resource");
            let l = self.o.link(r)?;
            let name = self.dev.pdev.fonts[fi].short_name.clone();
            self.pdf_doc_add_page_resource(b"Font", &name, l)?;
            self.dev.pdev.fonts[fi].used_on_this_page = true;
        }
        let font_scale = f64::from(self.dev.pdev.fonts[fi].sptsize) * self.dev.pdev.unit.dvi2pts;
        let mut b = Buf::new();
        b.extend(b" /");
        b.extend(&self.dev.pdev.fonts[fi].short_name);
        b.push(b' ');
        p_dtoa(
            &mut b,
            font_scale,
            (self.dev.pdev.unit.precision + 1).min(DEV_PRECISION_MAX) as usize,
        );
        b.extend(b" Tf");
        self.dev_out(&b.0)?;
        let bold = self.dev.pdev.fonts[fi].bold;
        if bold > 0.0 || bold != self.dev.pdev.text_state.bold_param {
            let mut b = Buf::new();
            if bold <= 0.0 {
                b.extend(b" 0 Tr");
            } else {
                b.extend(b" 2 Tr ");
                b.fixed(bold, 6);
                b.extend(b" w");
            }
            self.dev_out(&b.0)?;
        }
        self.dev.pdev.text_state.bold_param = bold;
        self.dev.pdev.text_state.font_id = font_id;
        Ok(())
    }

    /// `handle_multibyte_string`: status (0, or -1 when the CMap
    /// conversion failed) and the string to show (decoded through the
    /// dev font's CMap when it has one; C's `sbuf0`).
    fn handle_multibyte_string(&mut self, dev_font: usize, s: &[u8]) -> Result<(i32, Vec<u8>)> {
        let enc_id = self.dev.pdev.fonts[dev_font].enc_id;
        if enc_id < 0 {
            return Ok((0, s.to_vec()));
        }
        let mut out = vec![0u8; FORMAT_BUF_SIZE];
        let (mut inpos, mut outpos) = (0usize, 0usize);
        let mut inbytesleft = s.len() as i32;
        let mut outbytesleft = FORMAT_BUF_SIZE as i32;
        let cmap = self.CMap_cache_get(enc_id)?;
        self.CMap_decode(
            cmap,
            s,
            &mut inpos,
            &mut inbytesleft,
            &mut out,
            &mut outpos,
            &mut outbytesleft,
        )?;
        if inbytesleft != 0 {
            return Ok((-1, Vec::new()));
        }
        out.truncate(FORMAT_BUF_SIZE - outbytesleft as usize);
        Ok((0, out))
    }

    /// `pdf_dev_set_string`.
    pub fn pdf_dev_set_string(
        &mut self,
        xpos: Spt,
        ypos: Spt,
        instr: &[u8],
        width: Spt,
        font_id: i32,
    ) -> Result<()> {
        if font_id < 0 || font_id as usize >= self.dev.pdev.fonts.len() {
            fatal!("Invalid font: {} ({})", font_id, self.dev.pdev.fonts.len());
        }
        if font_id != self.dev.pdev.text_state.font_id {
            self.pdf_dev_set_font(font_id)?;
        }
        let fi = self.dev.pdev.text_state.font_id as usize;
        let text_xorigin = self.dev.pdev.text_state.ref_x;
        let text_yorigin = self.dev.pdev.text_state.ref_y;
        let format = self.dev.pdev.fonts[fi].format;
        let mut s: Vec<u8> = instr.to_vec();
        let used_chars = self.dev.pdev.fonts[fi].used_chars.clone();
        if format == PDF_FONTTYPE_COMPOSITE {
            let (r, out) = self.handle_multibyte_string(fi, &s)?;
            if r < 0 {
                crate::fatal!("Error in converting input string...");
            }
            s = out;
            if let Some(uc) = used_chars {
                let mut uc = uc.borrow_mut();
                let mut i = 0;
                while i + 1 < s.len() {
                    let cid = (u16::from(s[i]) << 8) | u16::from(s[i + 1]);
                    crate::pdffont::add_to_used_chars2(&mut uc, u32::from(cid));
                    i += 2;
                }
            }
        } else if let Some(uc) = used_chars {
            let mut uc = uc.borrow_mut();
            for &c in &s {
                uc[c as usize] = 1;
            }
        }
        let dir = self.dev.pdev.text_state.dir_mode;
        let offset = self.dev.pdev.text_state.offset;
        let (delh, delv) = if dir == 0 {
            (text_xorigin + offset - xpos, ypos - text_yorigin)
        } else if dir == 1 {
            (ypos - text_yorigin + offset, xpos - text_xorigin)
        } else {
            (ypos + text_yorigin + offset, xpos + text_xorigin)
        };
        let (extend, sptsize, slant, wmode) = {
            let f = &self.dev.pdev.fonts[fi];
            (f.extend, f.sptsize, f.slant, f.wmode)
        };
        let word_space_max = (3.0 * extend * f64::from(sptsize)) as Spt;
        let kern: Spt = if self.dev.pdev.text_state.force_reset
            || delv.abs() > self.dev.pdev.unit.min_bp_val
            || delh.abs() > word_space_max
        {
            self.pdf_dev_text_mode()?;
            0
        } else {
            (1000.0 / extend * f64::from(delh) / f64::from(sptsize)) as Spt
        };
        let mut b = Buf::new();
        if self.dev.pdev.motion_state != STRING_MODE {
            let rotate = self.dev.pdev.text_state.matrix.rotate;
            self.pdf_dev_string_mode(xpos, ypos, slant, extend, rotate)?;
        } else if kern != 0 {
            self.dev.pdev.text_state.offset -=
                (f64::from(kern) * extend * (f64::from(sptsize) / 1000.0)) as Spt;
            let is_mb = self.dev.pdev.text_state.is_mb;
            b.push(if is_mb { b'>' } else { b')' });
            if wmode != 0 {
                b.int(-kern);
            } else {
                b.int(kern);
            }
            b.push(if is_mb { b'<' } else { b'(' });
            self.dev_out(&b.0)?;
            b = Buf::new();
        }
        if self.dev.pdev.text_state.is_mb {
            for &c in &s {
                let first = (c >> 4) & 0x0f;
                let second = c & 0x0f;
                b.push(if first >= 10 {
                    first + b'W'
                } else {
                    first + b'0'
                });
                b.push(if second >= 10 {
                    second + b'W'
                } else {
                    second + b'0'
                });
            }
        } else {
            crate::obj::escape_str(&mut b, &s);
        }
        self.dev_out(&b.0)?;
        self.dev.pdev.text_state.offset += width;
        Ok(())
    }

    /// `pdf_init_device`.
    pub fn pdf_init_device(
        &mut self,
        dvi2pts: f64,
        precision: i32,
        black_and_white: i32,
    ) -> Result<()> {
        let d = &mut self.dev.pdev;
        d.motion_state = GRAPHICS_MODE;
        d.param.autorotate = 1;
        d.param.colormode = 1;
        d.text_state = TextState {
            font_id: -1,
            ..TextState::default()
        };
        d.unit.precision = precision.clamp(0, DEV_PRECISION_MAX);
        d.unit.dvi2pts = dvi2pts;
        d.unit.min_bp_val = round_acc(
            1.0 / (f64::from(TEN_POW[d.unit.precision as usize]) * dvi2pts),
            1.0,
        ) as i32;
        if d.unit.min_bp_val < 0 {
            d.unit.min_bp_val = -d.unit.min_bp_val;
        }
        d.param.colormode = i32::from(black_and_white == 0);
        self.pdf_dev_graphics_mode()?;
        self.pdf_color_clear_stack();
        self.pdf_dev_init_gstates();
        self.dev.pdev.fonts.clear();
        Ok(())
    }

    /// `pdf_close_device`.
    pub fn pdf_close_device(&mut self) -> Result<()> {
        let fonts = core::mem::take(&mut self.dev.pdev.fonts);
        for f in fonts {
            self.o.release_opt(f.resource)?;
        }
        self.pdf_dev_clear_gstates()?;
        Ok(())
    }

    /// `pdf_dev_reset_fonts`.
    pub fn pdf_dev_reset_fonts(&mut self, newpage: i32) {
        for f in &mut self.dev.pdev.fonts {
            f.used_on_this_page = false;
        }
        let ts = &mut self.dev.pdev.text_state;
        ts.font_id = -1;
        ts.matrix.slant = 0.0;
        ts.matrix.extend = 1.0;
        ts.matrix.rotate = TEXT_WMODE_HH;
        if newpage != 0 {
            ts.bold_param = 0.0;
        }
        ts.is_mb = false;
    }

    /// `pdf_dev_reset_color`.
    pub fn pdf_dev_reset_color(&mut self, force: i32) -> Result<()> {
        let (sc, fc) = self.pdf_color_get_current();
        self.pdf_dev_set_color(&sc, 0, force)?;
        self.pdf_dev_set_color(&fc, 0x20, force)?;
        Ok(())
    }

    /// `pdf_dev_bop`.
    pub fn pdf_dev_bop(&mut self, m: &PdfTmatrix) -> Result<()> {
        self.pdf_dev_graphics_mode()?;
        self.dev.pdev.text_state.force_reset = false;
        self.pdf_dev_gsave()?;
        self.pdf_dev_concat(m)?;
        self.pdf_dev_reset_fonts(1);
        self.pdf_dev_reset_color(0)?;
        self.pdf_dev_reset_xgstate(0)?;
        Ok(())
    }

    /// `pdf_dev_eop`.
    pub fn pdf_dev_eop(&mut self) -> Result<()> {
        self.pdf_dev_graphics_mode()?;
        let depth = self.pdf_dev_current_depth();
        if depth != 1 {
            self.pdf_dev_grestore_to(0)?;
        } else {
            self.pdf_dev_grestore()?;
        }
        Ok(())
    }

    /// `pdf_dev_locate_font`.
    pub fn pdf_dev_locate_font(&mut self, font_name: &[u8], ptsize: Spt) -> Result<i32> {
        if ptsize == 0 {
            fatal!("pdf_dev_locate_font() called with the zero ptsize.");
        }
        if let Some(i) = self
            .dev
            .pdev
            .fonts
            .iter()
            .position(|f| f.tex_name == font_name && f.sptsize == ptsize)
        {
            return Ok(i as i32);
        }
        let mrec = self.pdf_lookup_fontmap_record(font_name);
        let scale = f64::from(ptsize) * self.dev.pdev.unit.dvi2pts;
        let mut font_id = self.pdf_font_findresource(font_name, scale);
        if font_id < 0 {
            font_id = self.pdf_font_load_font(font_name, scale, mrec.as_ref())?;
            if font_id < 0 {
                return Ok(-1);
            }
        }
        let short_name = self.pdf_font_resource_name(font_id)?;
        let format = match self.pdf_get_font_subtype(font_id)? {
            crate::pdffont::PDF_FONT_FONTTYPE_TYPE3 => PDF_FONTTYPE_BITMAP,
            crate::pdffont::PDF_FONT_FONTTYPE_TYPE0 => PDF_FONTTYPE_COMPOSITE,
            _ => PDF_FONTTYPE_SIMPLE,
        };
        let wmode = self.pdf_get_font_wmode(font_id)?;
        let enc_id = self.pdf_get_font_encoding(font_id)?;
        let (extend, slant, bold) = match &mrec {
            Some(m) => (m.opt.extend, m.opt.slant, m.opt.bold),
            None => (1.0, 0.0, 0.0),
        };
        self.dev.pdev.fonts.push(DevFont {
            short_name,
            used_on_this_page: false,
            tex_name: font_name.to_vec(),
            sptsize: ptsize,
            font_id,
            enc_id,
            resource: None,
            used_chars: None,
            format,
            wmode,
            extend,
            slant,
            bold,
        });
        Ok((self.dev.pdev.fonts.len() - 1) as i32)
    }

    fn dev_sprint_line(&self, b: &mut Buf, width: Spt, p0x: Spt, p0y: Spt, p1x: Spt, p1y: Spt) {
        let w = f64::from(width) * self.dev.pdev.unit.dvi2pts;
        p_dtoa(
            b,
            w,
            (self.dev.pdev.unit.precision + 1).min(DEV_PRECISION_MAX) as usize,
        );
        b.extend(b" w ");
        self.dev_sprint_bp(b, p0x, None);
        b.push(b' ');
        self.dev_sprint_bp(b, p0y, None);
        b.extend(b" m ");
        self.dev_sprint_bp(b, p1x, None);
        b.push(b' ');
        self.dev_sprint_bp(b, p1y, None);
        b.extend(b" l S");
    }

    /// `pdf_dev_set_rule`.
    pub fn pdf_dev_set_rule(
        &mut self,
        xpos: Spt,
        ypos: Spt,
        width: Spt,
        height: Spt,
    ) -> Result<()> {
        self.pdf_dev_graphics_mode()?;
        let mut b = Buf::new();
        b.extend(b" q ");
        let width_in_bp = f64::from(width.min(height)) * self.dev.pdev.unit.dvi2pts;
        if !(0.0..=PDF_LINE_THICKNESS_MAX).contains(&width_in_bp) {
            let d = self.dev.pdev.unit.dvi2pts;
            let rect = PdfRect {
                llx: d * f64::from(xpos),
                lly: d * f64::from(ypos),
                urx: d * f64::from(width),
                ury: d * f64::from(height),
            };
            self.pdf_sprint_rect(&mut b, &rect);
            b.extend(b" re f");
        } else if width > height {
            self.dev_sprint_line(
                &mut b,
                height,
                xpos,
                ypos + height / 2,
                xpos + width,
                ypos + height / 2,
            );
        } else {
            self.dev_sprint_line(
                &mut b,
                width,
                xpos + width / 2,
                ypos,
                xpos + width / 2,
                ypos + height,
            );
        }
        b.extend(b" Q");
        self.dev_out(&b.0)?;
        Ok(())
    }

    /// `pdf_dev_set_rect`: a box in device space.
    pub fn pdf_dev_set_rect(
        &mut self,
        x_user: Spt,
        y_user: Spt,
        width: Spt,
        height: Spt,
        depth: Spt,
    ) -> PdfRect {
        let d = self.dev.pdev.unit.dvi2pts;
        let dev_x = f64::from(x_user) * d;
        let dev_y = f64::from(y_user) * d;
        let (mut p0, mut p1, mut p2, mut p3);
        if self.dev.pdev.text_state.dir_mode != 0 {
            p0 = PdfCoord {
                x: dev_x - d * f64::from(depth),
                y: dev_y - d * f64::from(width),
            };
            p1 = PdfCoord {
                x: dev_x + d * f64::from(height),
                y: p0.y,
            };
            p2 = PdfCoord { x: p1.x, y: dev_y };
            p3 = PdfCoord { x: p0.x, y: p2.y };
        } else {
            p0 = PdfCoord {
                x: dev_x,
                y: dev_y - d * f64::from(depth),
            };
            p1 = PdfCoord {
                x: dev_x + d * f64::from(width),
                y: p0.y,
            };
            p2 = PdfCoord {
                x: p1.x,
                y: dev_y + d * f64::from(height),
            };
            p3 = PdfCoord { x: p0.x, y: p2.y };
        }
        self.pdf_dev_transform(&mut p0, None);
        self.pdf_dev_transform(&mut p1, None);
        self.pdf_dev_transform(&mut p2, None);
        self.pdf_dev_transform(&mut p3, None);
        let min = |a: f64, b: f64| if a < b { a } else { b };
        let max = |a: f64, b: f64| if a > b { a } else { b };
        PdfRect {
            llx: min(min(min(p0.x, p1.x), p2.x), p3.x),
            lly: min(min(min(p0.y, p1.y), p2.y), p3.y),
            urx: max(max(max(p0.x, p1.x), p2.x), p3.x),
            ury: max(max(max(p0.y, p1.y), p2.y), p3.y),
        }
    }

    /// `pdf_dev_get_dirmode`.
    #[must_use]
    pub fn pdf_dev_get_dirmode(&self) -> i32 {
        self.dev.pdev.text_state.dir_mode
    }

    /// `pdf_dev_set_dirmode`.
    pub fn pdf_dev_set_dirmode(&mut self, text_dir: i32) {
        let font = self.dev.pdev.text_state.font_id;
        // (C's CURRENTFONT: fonts[font_id], NULL before any font)
        let wmode = if font >= 0 {
            self.dev
                .pdev
                .fonts
                .get(font as usize)
                .map_or(0, |f| f.wmode)
        } else {
            0
        };
        let vert_font = i32::from(wmode != 0);
        let vert_dir = if self.dev.pdev.param.autorotate != 0 {
            text_dir
        } else {
            vert_font
        };
        let text_rotate = (vert_font << 2) | vert_dir;
        if font >= 0
            && (font as usize) < self.dev.pdev.fonts.len()
            && angle_changes(text_rotate, self.dev.pdev.text_state.matrix.rotate)
        {
            self.dev.pdev.text_state.force_reset = true;
        }
        self.dev.pdev.text_state.matrix.rotate = text_rotate;
        self.dev.pdev.text_state.dir_mode = text_dir;
    }

    fn dev_set_param_autorotate(&mut self, auto_rotate: i32) {
        let font = self.dev.pdev.text_state.font_id;
        let wmode = if font >= 0 {
            self.dev
                .pdev
                .fonts
                .get(font as usize)
                .map_or(0, |f| f.wmode)
        } else {
            0
        };
        let vert_font = i32::from(wmode != 0);
        let vert_dir = if auto_rotate != 0 {
            self.dev.pdev.text_state.dir_mode
        } else {
            vert_font
        };
        let text_rotate = (vert_font << 2) | vert_dir;
        if angle_changes(text_rotate, self.dev.pdev.text_state.matrix.rotate) {
            self.dev.pdev.text_state.force_reset = true;
        }
        self.dev.pdev.text_state.matrix.rotate = text_rotate;
        self.dev.pdev.param.autorotate = auto_rotate;
    }

    /// `pdf_dev_get_param`.
    #[must_use]
    pub fn pdf_dev_get_param(&self, param_type: i32) -> i32 {
        match param_type {
            PDF_DEV_PARAM_AUTOROTATE => self.dev.pdev.param.autorotate,
            PDF_DEV_PARAM_COLORMODE => self.dev.pdev.param.colormode,
            _ => panic!("Unknown device parameter"),
        }
    }

    /// `pdf_dev_set_param`.
    pub fn pdf_dev_set_param(&mut self, param_type: i32, value: i32) {
        match param_type {
            PDF_DEV_PARAM_AUTOROTATE => self.dev_set_param_autorotate(value),
            PDF_DEV_PARAM_COLORMODE => self.dev.pdev.param.colormode = value,
            _ => panic!("Unknown device parameter"),
        }
    }

    /// `pdf_dev_put_image`: the image's box in device space if asked.
    pub fn pdf_dev_put_image(
        &mut self,
        id: i32,
        ti: &mut TransformInfo,
        ref_x: f64,
        ref_y: f64,
    ) -> Result<(i32, PdfRect)> {
        let mut m = ti.matrix;
        m.e += ref_x;
        m.f += ref_y;
        if self.dev.pdev.param.autorotate != 0 && self.dev.pdev.text_state.dir_mode != 0 {
            let tmp = -m.a;
            m.a = m.b;
            m.b = tmp;
            let tmp = -m.c;
            m.c = m.d;
            m.d = tmp;
        }
        self.pdf_dev_graphics_mode()?;
        self.pdf_dev_gsave()?;
        let (_, m1, r) = self.pdf_ximage_scale_image(id, ti)?;
        crate::pdfdraw::pdf_concatmatrix(&mut m, &m1);
        self.pdf_dev_concat(&m)?;
        if ti.flags & INFO_DO_CLIP != 0 {
            self.pdf_dev_rectclip(r.llx, r.lly, r.urx - r.llx, r.ury - r.lly)?;
        }
        let res_name = self.pdf_ximage_get_resname(id)?;
        let mut b = Buf::new();
        b.extend(b" /");
        b.extend(&res_name);
        b.extend(b" Do");
        self.dev_out(&b.0)?;
        let rect = {
            let (x, y) = (self.bpt2spt(r.llx), self.bpt2spt(r.lly));
            let (w, h) = (self.bpt2spt(r.urx - r.llx), self.bpt2spt(r.ury - r.lly));
            self.pdf_dev_set_rect(x, y, w, h, 0)
        };
        self.pdf_dev_grestore()?;
        let r = self.pdf_ximage_get_reference(id)?;
        self.pdf_doc_add_page_resource(b"XObject", &res_name, r)?;
        Ok((0, rect))
    }

    /// `pdf_dev_begin_actualtext`.
    pub fn pdf_dev_begin_actualtext(&mut self, unicodes: &[u16]) -> Result<()> {
        let pdf_doc_enc = !unicodes.iter().any(|&u| u > 0xff || (u > 0x7f && u < 0xa1));
        self.pdf_dev_graphics_mode()?;
        self.dev_out(b"\n/Span << /ActualText (")?;
        if !pdf_doc_enc {
            self.dev_out(b"\xFE\xFF")?;
        }
        for &u in unicodes {
            let s = [(u >> 8) as u8, u as u8];
            let mut b = Buf::new();
            for &c in &s[usize::from(pdf_doc_enc)..] {
                if c == b'(' || c == b')' || c == b'\\' {
                    b.push(b'\\');
                    b.push(c);
                } else if c < b' ' {
                    b.push(b'\\');
                    b.push(b'0' + (c >> 6));
                    b.push(b'0' + ((c >> 3) & 7));
                    b.push(b'0' + (c & 7));
                } else {
                    b.push(c);
                }
            }
            self.dev_out(&b.0)?;
        }
        self.dev_out(b") >> BDC")?;
        Ok(())
    }

    /// `pdf_dev_end_actualtext`.
    pub fn pdf_dev_end_actualtext(&mut self) -> Result<()> {
        self.pdf_dev_graphics_mode()?;
        self.dev_out(b" EMC")?;
        Ok(())
    }

    /// `graphics_mode`.
    pub fn graphics_mode(&mut self) -> Result<()> {
        self.pdf_dev_graphics_mode()?;
        Ok(())
    }

    /// `dev_unit_dviunit`.
    #[must_use]
    pub fn dev_unit_dviunit(&self) -> f64 {
        1.0 / self.dev.pdev.unit.dvi2pts
    }

    /// `pdf_dev_get_font_wmode`.
    #[must_use]
    pub fn pdf_dev_get_font_wmode(&self, font_id: i32) -> i32 {
        self.dev
            .pdev
            .fonts
            .get(font_id as usize)
            .map_or(0, |f| f.wmode)
    }

    /// `pdf_dev_font_minbytes`.
    pub fn pdf_dev_font_minbytes(&mut self, font_id: i32) -> Result<i32> {
        match self.dev.pdev.fonts.get(font_id as usize) {
            Some(f) if f.format == PDF_FONTTYPE_COMPOSITE => {
                let enc = f.enc_id;
                let cmap = self.CMap_cache_get(enc)?;
                cmap.CMap_get_profile(crate::cmap::CMAP_PROF_TYPE_INBYTES_MIN)
            }
            _ => Ok(1),
        }
    }
}
