//! Type 3 fonts from PK files: pdfTeX's `writet3.c` (its PK path,
//! `writepk`, and the font dictionary `writet3` writes after it) and
//! `pkin.c` (`readchar`, the PK file read as dvips reads it), with the
//! pieces of pdfTeX they use: `get_pk_font_scale`, `pk_char_width` and
//! `getpkcharwidth` (`adv_char_width` for a font that is not scalable,
//! pdfTeX §726), the PK resolution fixed at the first shipout (pdfTeX
//! §792) and kpathsea's `kpse_magstep_fix` (`magstep.c`), which names
//! the PK file a font asks for.
//!
//! pdfTeX looks for a `.pgc` file (a font as PDF glyph code) before the
//! PK file; this writer does not, so such a font is written from its PK
//! file.

// (`pkin.c`'s and `writet3.c`'s arithmetic and loops, kept as they are
// for reading against them)
#![allow(
    clippy::needless_range_loop,
    clippy::cast_sign_loss,
    clippy::comparison_chain
)]

use std::sync::Arc;

use phitex_typeset::arith::Scaled;
use phitex_typeset::fonts::FontId;

use crate::map::MapEntry;
use crate::num::{ONE_HUNDRED_BP, divide_scaled};
use crate::{PdfError, PdfState};

/// `pk_dpi`: the PK resolution where `\pdfpkresolution` is 0 (web2c's
/// default, `texmf.cnf` sets none).
pub const PK_DPI_DEFAULT: i32 = 72;

/// `fix_int(pdf_pk_resolution, 72, 8000)` with 0 read as `pk_dpi`
/// (pdfTeX §792): the resolution PK fonts are asked for at.
#[must_use]
pub fn fixed_pk_resolution(pk_resolution: i32) -> i32 {
    let r = if pk_resolution == 0 {
        PK_DPI_DEFAULT
    } else {
        pk_resolution
    };
    r.clamp(72, 8000)
}

/// web2c's `zround` (`lib/zround.c`), which `round` is in pdfTeX's C.
#[allow(clippy::cast_possible_truncation)]
fn zround(r: f64) -> i32 {
    if r > 2_147_483_647.0 {
        2_147_483_647
    } else if r < -2_147_483_647.0 {
        -2_147_483_647
    } else if r >= 0.0 {
        (r + 0.5) as i32
    } else {
        (r - 0.5) as i32
    }
}

/// kpathsea's `magstep` (`magstep.c`): magstep `n` (its lowest bit a
/// half step) of resolution `bdpi`.
#[allow(clippy::cast_possible_truncation)]
fn magstep(mut n: i32, bdpi: i32) -> i32 {
    let neg = n < 0;
    if neg {
        n = -n;
    }
    let mut t: f64 = if n & 1 != 0 {
        n &= !1;
        1.095_445_115
    } else {
        1.0
    };
    while n > 8 {
        n -= 8;
        t *= 2.0736;
    }
    while n > 0 {
        n -= 2;
        t *= 1.2;
    }
    (0.5 + if neg {
        f64::from(bdpi) / t
    } else {
        f64::from(bdpi) * t
    }) as i32
}

/// kpathsea's `kpse_magstep_fix` (`magstep.c`): the true resolution of
/// `dpi` at base resolution `bdpi`, the magstep within one of it.
#[must_use]
pub fn magstep_fix(dpi: u32, bdpi: u32) -> u32 {
    let sign = if dpi < bdpi { -1 } else { 1 };
    let (d, b) = (
        i32::try_from(dpi).unwrap_or(i32::MAX),
        i32::try_from(bdpi).unwrap_or(i32::MAX),
    );
    let mut real_dpi: u32 = 0;
    let mut m = 0;
    while real_dpi == 0 && m < 40 {
        let mdpi = magstep(m * sign, b);
        if (mdpi - d).abs() <= 1 {
            real_dpi = u32::try_from(mdpi).unwrap_or(0);
        } else if (mdpi - d) * sign > 0 {
            real_dpi = dpi;
        }
        m += 1;
    }
    if real_dpi != 0 { real_dpi } else { dpi }
}

/// The resolution `writepk` asks for font of size `size` (scaled
/// points) and design size `design_size` at PK resolution
/// `fixed_pk_resolution` ([`fixed_pk_resolution`]):
/// `kpse_magstep_fix(round(res * (float) pdf_font_size / font_dsize), res)`,
/// in C's single precision as `writet3.c` computes it.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn pk_dpi(size: Scaled, design_size: Scaled, fixed_pk_resolution: i32) -> u32 {
    // (`pdf_font_size`, `pdf_use_font`)
    let pdf_size = divide_scaled(size, ONE_HUNDRED_BP, 6).1;
    let r = fixed_pk_resolution as f32 * (pdf_size as f32 / design_size as f32);
    let dpi = u32::try_from(zround(f64::from(r))).unwrap_or(0);
    magstep_fix(dpi, u32::try_from(fixed_pk_resolution).unwrap_or(0))
}

/// The name of the PK file of font `name` at `dpi` (kpathsea's
/// `$KPATHSEA_NAME.$KPATHSEA_DPI$KPATHSEA_FORMAT`): `cmr10.600pk`.
#[must_use]
pub fn pk_file_name(name: &[u8], dpi: u32) -> Vec<u8> {
    let mut n = name.to_vec();
    n.extend_from_slice(format!(".{dpi}pk").as_bytes());
    n
}

/// The PK file a writer asks a finder for: [`pk_file_name`]'s
/// `NAME.DPIpk` and the glyph settings pdfTeX gives `kpse_init_prog`,
/// base resolution `bdpi` (`MAKETEX_BASE_DPI`) and mode `mode`
/// (`MAKETEX_MODE`), which kpathsea's search path and mktexpk go by:
/// `cmr10.600pk@600`, `cmr10.600pk@600:ljfour` (`phitex_kpse::pk_settings`
/// reads them back).
#[must_use]
pub fn pk_request(name: &[u8], dpi: u32, bdpi: u32, mode: Option<&str>) -> String {
    let file = String::from_utf8_lossy(&pk_file_name(name, dpi)).into_owned();
    match mode.filter(|m| !m.is_empty()) {
        Some(m) => format!("{file}@{bdpi}:{m}"),
        None => format!("{file}@{bdpi}"),
    }
}

/// `pdftex_fail` while `writepk` reads a PK file: `cur_file_name` the
/// font's name.
fn fail(m: impl Into<String>) -> PdfError {
    PdfError::Fail {
        file: None,
        message: m.into(),
    }
}

/// `chardesc` (`ptexlib.h`).
#[derive(Default)]
struct CharDesc {
    charcode: i32,
    cwidth: i32,
    cheight: i32,
    xoff: i32,
    yoff: i32,
    /// The rows, 16 pixels a word, each row whole words.
    raster: Vec<i32>,
}

/// Which `realfunc` `pkin.c` calls for the next count.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RealFunc {
    PackedNum,
    Rest,
}

/// `pkin.c`'s reader over a PK file's bytes.
struct PkIn<'a> {
    pk: &'a [u8],
    pos: usize,
    inputbyte: i32,
    flagbyte: i32,
    bitweight: i32,
    dynf: i32,
    repeatcount: i32,
    realfunc: RealFunc,
    remainder: i64,
}

impl<'a> PkIn<'a> {
    fn new(pk: &'a [u8]) -> Self {
        PkIn {
            pk,
            pos: 0,
            inputbyte: 0,
            flagbyte: 0,
            bitweight: 0,
            dynf: 0,
            repeatcount: 0,
            realfunc: RealFunc::PackedNum,
            remainder: 0,
        }
    }

    fn pkbyte(&mut self) -> Result<i32, PdfError> {
        let b = self
            .pk
            .get(self.pos)
            .ok_or_else(|| fail("unexpected eof in pk file"))?;
        self.pos += 1;
        Ok(i32::from(*b))
    }

    fn signed(&mut self, n: usize) -> Result<i32, PdfError> {
        let mut i = self.pkbyte()?;
        if i > 127 {
            i -= 256;
        }
        for _ in 1..n {
            i = i.wrapping_mul(256).wrapping_add(self.pkbyte()?);
        }
        Ok(i)
    }

    fn pkduo(&mut self) -> Result<i32, PdfError> {
        self.signed(2)
    }

    fn pktrio(&mut self) -> Result<i32, PdfError> {
        self.signed(3)
    }

    fn pkquad(&mut self) -> Result<i32, PdfError> {
        self.signed(4)
    }

    fn getnyb(&mut self) -> Result<i32, PdfError> {
        if self.bitweight == 0 {
            self.bitweight = 16;
            self.inputbyte = self.pkbyte()?;
            Ok(self.inputbyte >> 4)
        } else {
            self.bitweight = 0;
            Ok(self.inputbyte & 15)
        }
    }

    fn getbit(&mut self) -> Result<bool, PdfError> {
        self.bitweight >>= 1;
        if self.bitweight == 0 {
            self.inputbyte = self.pkbyte()?;
            self.bitweight = 128;
        }
        Ok(self.inputbyte & self.bitweight != 0)
    }

    fn realfunc(&mut self) -> Result<i32, PdfError> {
        match self.realfunc {
            RealFunc::PackedNum => self.pkpackednum(),
            RealFunc::Rest => self.rest(),
        }
    }

    fn pkpackednum(&mut self) -> Result<i32, PdfError> {
        let mut i = self.getnyb()?;
        if i == 0 {
            let mut j;
            loop {
                j = self.getnyb()?;
                i += 1;
                if j != 0 {
                    break;
                }
            }
            if i > 3 {
                // a huge count, faked by an artificially large repeat
                // count
                return self.handlehuge(i, j);
            }
            while i > 0 {
                j = j * 16 + self.getnyb()?;
                i -= 1;
            }
            Ok(j - 15 + (13 - self.dynf) * 16 + self.dynf)
        } else if i <= self.dynf {
            Ok(i)
        } else if i < 14 {
            Ok((i - self.dynf - 1) * 16 + self.getnyb()? + self.dynf + 1)
        } else {
            self.repeatcount = if i == 14 { self.pkpackednum()? } else { 1 };
            self.realfunc()
        }
    }

    #[allow(clippy::cast_possible_truncation)]
    fn rest(&mut self) -> Result<i32, PdfError> {
        if self.remainder < 0 {
            self.remainder = -self.remainder;
            Ok(0)
        } else if self.remainder > 0 {
            if self.remainder > 4000 {
                self.remainder = 4000 - self.remainder;
                Ok(4000)
            } else {
                let i = self.remainder as i32;
                self.remainder = 0;
                self.realfunc = RealFunc::PackedNum;
                Ok(i)
            }
        } else {
            Err(fail("shouldn't happen"))
        }
    }

    fn handlehuge(&mut self, mut i: i32, k: i32) -> Result<i32, PdfError> {
        let mut j = i64::from(k);
        while i != 0 {
            j = (j << 4) + i64::from(self.getnyb()?);
            i -= 1;
        }
        self.remainder = j - 15 + i64::from((13 - self.dynf) * 16 + self.dynf);
        self.realfunc = RealFunc::Rest;
        self.rest()
    }

    // (the raster grows as it is written, where `pkin.c` allocates
    // `2 * cheight * wordwidth` words and trusts the file)
    fn unpack(&mut self, cd: &mut CharDesc) -> Result<(), PdfError> {
        const GPOWER: [i32; 17] = [
            0, 1, 3, 7, 15, 31, 63, 127, 255, 511, 1023, 2047, 4095, 8191, 16383, 32767, 65535,
        ];
        let wordwidth = (cd.cwidth + 15) / 16;
        let ww = usize::try_from(wordwidth).unwrap_or(0);
        cd.raster.clear();
        self.realfunc = RealFunc::PackedNum;
        self.dynf = self.flagbyte / 16;
        let mut turnon = self.flagbyte & 8 != 0;
        if self.dynf == 14 {
            self.bitweight = 0;
            for _ in 1..=cd.cheight {
                let mut word = 0;
                let mut wordweight = 32768;
                for _ in 1..=cd.cwidth {
                    if self.getbit()? {
                        word += wordweight;
                    }
                    wordweight >>= 1;
                    if wordweight == 0 {
                        cd.raster.push(word);
                        word = 0;
                        wordweight = 32768;
                    }
                }
                if wordweight != 32768 {
                    cd.raster.push(word);
                }
            }
        } else {
            let mut rowsleft = cd.cheight;
            let mut hbit = cd.cwidth;
            self.repeatcount = 0;
            let mut wordweight: i32 = 16;
            let mut word = 0;
            self.bitweight = 0;
            let gp = |k: i32| GPOWER[usize::try_from(k).unwrap_or(0).min(16)];
            while rowsleft > 0 {
                let mut count = self.realfunc()?;
                while count != 0 {
                    if count < wordweight && count < hbit {
                        if turnon {
                            word += gp(wordweight) - gp(wordweight - count);
                        }
                        hbit -= count;
                        wordweight -= count;
                        count = 0;
                    } else if count >= hbit && hbit <= wordweight {
                        if turnon {
                            word += gp(wordweight) - gp(wordweight - hbit);
                        }
                        cd.raster.push(word);
                        for _ in 1..=self.repeatcount {
                            for _ in 1..=wordwidth {
                                let prev = cd
                                    .raster
                                    .len()
                                    .checked_sub(ww)
                                    .and_then(|k| cd.raster.get(k).copied())
                                    .ok_or_else(|| {
                                        fail("error while unpacking; more bits than required")
                                    })?;
                                cd.raster.push(prev);
                            }
                        }
                        rowsleft -= self.repeatcount + 1;
                        self.repeatcount = 0;
                        word = 0;
                        wordweight = 16;
                        count -= hbit;
                        hbit = cd.cwidth;
                    } else {
                        if turnon {
                            word += gp(wordweight);
                        }
                        cd.raster.push(word);
                        word = 0;
                        count -= wordweight;
                        hbit -= wordweight;
                        wordweight = 16;
                    }
                }
                turnon = !turnon;
            }
            if rowsleft != 0 || hbit != cd.cwidth {
                return Err(fail("error while unpacking; more bits than required"));
            }
        }
        Ok(())
    }

    /// `readchar`: the next character's description, `None` at the
    /// postamble.
    fn readchar(&mut self, check_preamble: bool, cd: &mut CharDesc) -> Result<bool, PdfError> {
        if check_preamble {
            if self.pkbyte()? != 247 {
                return Err(fail("bad pk file, expected pre"));
            }
            if self.pkbyte()? != 89 {
                return Err(fail("bad version of pk file"));
            }
            let mut i = self.pkbyte()?;
            while i > 0 {
                self.pkbyte()?;
                i -= 1;
            }
            for _ in 0..4 {
                self.pkquad()?; // design size, checksum, hppp, vppp
            }
        }
        loop {
            self.flagbyte = self.pkbyte()?;
            if self.flagbyte == 245 {
                return Ok(false);
            }
            if self.flagbyte < 240 {
                let length: i64;
                match self.flagbyte & 7 {
                    0..=3 => {
                        length = i64::from((self.flagbyte & 7) * 256 + self.pkbyte()? - 3);
                        cd.charcode = self.pkbyte()?;
                        self.pktrio()?; // TFM width
                        self.pkbyte()?; // pixel width
                        cd.cwidth = self.pkbyte()?;
                        cd.cheight = self.pkbyte()?;
                        cd.xoff = self.pkbyte()?;
                        cd.yoff = self.pkbyte()?;
                        if cd.xoff > 127 {
                            cd.xoff -= 256;
                        }
                        if cd.yoff > 127 {
                            cd.yoff -= 256;
                        }
                    }
                    4..=6 => {
                        let l =
                            i64::from(self.flagbyte & 3) * 65536 + i64::from(self.pkbyte()?) * 256;
                        length = l + i64::from(self.pkbyte()?) - 4;
                        cd.charcode = self.pkbyte()?;
                        self.pktrio()?;
                        self.pkduo()?;
                        cd.cwidth = self.pkduo()?;
                        cd.cheight = self.pkduo()?;
                        cd.xoff = self.pkduo()?;
                        cd.yoff = self.pkduo()?;
                    }
                    _ => {
                        length = i64::from(self.pkquad()?) - 9;
                        cd.charcode = self.pkquad()?;
                        self.pkquad()?; // TFM width
                        self.pkquad()?; // pixel width
                        self.pkquad()?;
                        cd.cwidth = self.pkquad()?;
                        cd.cheight = self.pkquad()?;
                        cd.xoff = self.pkquad()?;
                        cd.yoff = self.pkquad()?;
                    }
                }
                if length <= 0 {
                    return Err(fail(format!("packet length ({length}) too small")));
                }
                self.unpack(cd)?;
                return Ok(true);
            }
            let mut k: i32 = 0;
            match self.flagbyte {
                240..=243 => {
                    if self.flagbyte == 243 {
                        k = self.pkbyte()?;
                        if k > 127 {
                            k -= 256;
                        }
                    }
                    if self.flagbyte >= 242 {
                        k = k.wrapping_mul(256).wrapping_add(self.pkbyte()?);
                    }
                    if self.flagbyte >= 241 {
                        k = k.wrapping_mul(256).wrapping_add(self.pkbyte()?);
                    }
                    k = k.wrapping_mul(256).wrapping_add(self.pkbyte()?);
                    while k > 0 {
                        k -= 1;
                        self.pkbyte()?;
                    }
                }
                244 => {
                    self.pkquad()?;
                }
                246 => {}
                _ => {
                    return Err(fail(format!("unexpected command ({})", self.flagbyte)));
                }
            }
        }
    }
}

/// What `writet3` keeps across fonts: `t3_b0` ... `t3_b3`, static in
/// `writet3.c` (a font none of whose characters is written keeps the
/// bounding box of the font before it).
#[derive(Default)]
pub(crate) struct T3Bbox([i32; 4]);

impl PdfState {
    /// `get_pk_font_scale`.
    fn pk_font_scale(&self, f: FontId) -> i32 {
        let size = self.font(f).size;
        divide_scaled(
            self.pk_scale_factor,
            divide_scaled(size, ONE_HUNDRED_BP, self.dd + 2).0,
            0,
        )
        .0
    }

    /// `pk_char_width`.
    fn pk_char_width(&self, f: FontId, w: Scaled) -> i32 {
        let size = self.font(f).size;
        divide_scaled(divide_scaled(w, size, 7).0, self.pk_font_scale(f), 0).0
    }

    /// `getpkcharwidth`: what `adv_char_width` advances `pdf_delta_h` by
    /// for a character of width `w` in font `f`, not scalable.
    #[allow(clippy::cast_possible_truncation)]
    pub(crate) fn pk_char_advance(&self, f: FontId, w: Scaled) -> Scaled {
        (f64::from(self.pk_font_scale(f)) / 100_000.0
            * (f64::from(self.pk_char_width(f, w)) / 100.0)
            * f64::from(self.font(f).size)) as Scaled
    }

    /// The resolution font `f`'s PK file is asked for at, with the PK
    /// resolution as it is fixed (or, before the first shipout, as
    /// `\pdfpkresolution` is now: what the caller found the file with).
    pub(crate) fn pk_dpi_of(&self, f: FontId) -> u32 {
        let res = if self.initialized {
            self.fixed_pk_resolution
        } else {
            fixed_pk_resolution(self.p.pk_resolution)
        };
        let m = &self.font(f).metrics;
        pk_dpi(m.size, m.design_size, res)
    }

    /// `writet3` for a PK font (`writepk`, then the font dictionary):
    /// font `f` (object `objnum`) with map entry `fm`, if any.
    #[allow(clippy::too_many_lines)]
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    pub(crate) fn writet3(
        &mut self,
        fm: Option<&MapEntry>,
        objnum: usize,
        f: FontId,
        bbox: &mut T3Bbox,
    ) -> Result<(), PdfError> {
        let mut t3_char_procs = [0usize; 256];
        let mut t3_char_widths = [0f32; 256];
        let mut t3_glyph_num = 0;
        // (`fe`: the map entry's encoding, its glyph names)
        let glyph_names = match fm.and_then(|m| m.encname.clone()) {
            Some(encname) => {
                let Some(bytes) = self.font(f).enc.clone() else {
                    return Err(PdfError::Font(format!(
                        "cannot open encoding file for reading: {}",
                        String::from_utf8_lossy(&encname)
                    )));
                };
                let mut g = crate::enc::load_enc(&bytes)?;
                // remove_duplicate_glyph_names
                let mut seen = std::collections::BTreeSet::new();
                for n in &mut g {
                    if let Some(name) = n.clone()
                        && name != b".notdef"
                        && !seen.insert(name)
                    {
                        *n = None;
                    }
                }
                Some((encname, g))
            }
            None => None,
        };
        // writepk
        let dpi = self.pk_dpi_of(f);
        let name = self.font(f).name.clone();
        let pk: Arc<[u8]> = match &self.font(f).pk {
            Some((d, Some(b))) if *d == dpi => b.clone(),
            _ => return Err(self.pk_not_found(f)),
        };
        let mut rd = PkIn::new(&pk);
        let mut cd = CharDesc::default();
        let mut check_preamble = true;
        let named = |e: PdfError| match e {
            PdfError::Fail {
                file: None,
                message,
            } => PdfError::Fail {
                file: Some(String::from_utf8_lossy(&name).into_owned()),
                message,
            },
            e => e,
        };
        while rd.readchar(check_preamble, &mut cd).map_err(named)? {
            check_preamble = false;
            let Ok(c) = usize::try_from(cd.charcode) else {
                continue;
            };
            if c > 255 || !self.font(f).chars[c] {
                continue;
            }
            let w = self.font(f).width(u16::try_from(c).unwrap_or(0));
            t3_char_widths[c] = self.pk_char_width(f, w) as f32;
            let is_null_glyph = cd.cwidth < 1 || cd.cheight < 1;
            if is_null_glyph {
                cd.cwidth = zround(f64::from(t3_char_widths[c]) / 100.0);
                cd.cheight = 1;
                cd.xoff = 0;
                cd.yoff = 0;
            }
            let llx = -cd.xoff;
            let lly = cd.yoff - cd.cheight + 1;
            let urx = cd.cwidth + llx + 1;
            let ury = cd.cheight + lly;
            // update_bbox
            let b = &mut bbox.0;
            if t3_glyph_num == 0 {
                *b = [llx, lly, urx, ury];
            } else {
                b[0] = b[0].min(llx);
                b[1] = b[1].min(lly);
                b[2] = b[2].max(urx);
                b[3] = b[3].max(ury);
            }
            t3_glyph_num += 1;
            t3_char_procs[c] = self.new_dict(crate::OBJ_TYPE_OTHERS, 0, 0);
            self.begin_stream();
            self.print_real(t3_char_widths[c] as i32, 2);
            self.print(format!(" 0 {llx} {lly} {urx} {ury} d1\n").as_bytes());
            if !is_null_glyph {
                self.print(
                    format!(
                        "q\n{} 0 0 {} {llx} {lly} cm\nBI\n/W {}\n/H {}\n",
                        cd.cwidth, cd.cheight, cd.cwidth, cd.cheight
                    )
                    .as_bytes(),
                );
                self.print(b"/IM true\n/BPC 1\n/D [1 0]\nID ");
                let cw = (cd.cwidth + 7) / 8;
                let rw = (cd.cwidth + 15) / 16;
                let mut row = 0usize;
                let word = |k: usize| cd.raster.get(k).copied().unwrap_or(0);
                for _ in 0..cd.cheight {
                    for _ in 0..rw - 1 {
                        self.pdf_out((word(row) / 256) as u8);
                        self.pdf_out((word(row) % 256) as u8);
                        row += 1;
                    }
                    self.pdf_out((word(row) / 256) as u8);
                    if 2 * rw == cw {
                        self.pdf_out((word(row) % 256) as u8);
                    }
                    row += 1;
                }
                self.print(b"\nEI\nQ\n");
            }
            self.end_stream();
        }
        // write_font_dict
        let (first_char, last_char) = self.char_range(f);
        let tounicode = match &glyph_names {
            Some((encname, g))
                if self.fixed_gen_tounicode > 0 && !self.font(f).nobuiltin_tounicode =>
            {
                let tfm = fm.map(|m| m.tfm_name.clone()).unwrap_or_default();
                self.write_tounicode(g, &tfm, Some(encname))
            }
            _ => 0,
        };
        let names = glyph_names.map(|(_, g)| g);
        let glyph = |i: usize| -> Option<Vec<u8>> {
            names
                .as_ref()
                .and_then(|g| g[i].clone())
                .filter(|n| n != b".notdef")
        };
        self.begin_dict(objnum, 1);
        self.print(b"/Type /Font\n/Subtype /Type3\n");
        let tag = self.font(f).tag.unwrap_or(f);
        self.print(format!("/Name /F{tag}\n").as_bytes());
        let attr = self.font(f).attr.clone();
        if !attr.is_empty() {
            self.print(&attr);
            self.print(b"\n");
        }
        let pk_font_scale = self.pk_font_scale(f);
        self.print(b"/FontMatrix [");
        self.print_real(pk_font_scale, 5);
        self.print(b" 0 0 ");
        self.print_real(pk_font_scale, 5);
        self.print(b" 0 0]\n");
        let b = bbox.0;
        self.print(format!("/FontBBox [ {} {} {} {} ]\n", b[0], b[1], b[2], b[3]).as_bytes());
        // (`t3_image_used`: a PK font's always)
        self.print(b"/Resources << /ProcSet [ /PDF /ImageB ] >>\n");
        self.print(format!("/FirstChar {first_char}\n/LastChar {last_char}\n").as_bytes());
        let wptr = self.new_objnum_();
        let eptr = self.new_objnum_();
        let cptr = self.new_objnum_();
        self.print(
            format!("/Widths {wptr} 0 R\n/Encoding {eptr} 0 R\n/CharProcs {cptr} 0 R\n").as_bytes(),
        );
        if tounicode != 0 {
            self.print(format!("/ToUnicode {tounicode} 0 R\n").as_bytes());
        }
        self.end_dict();
        self.begin_obj(wptr, 1);
        self.print(b"[");
        for w in &t3_char_widths[first_char..=last_char] {
            self.print_real(*w as i32, 2);
            self.print(b" ");
        }
        self.print(b"]\n");
        self.end_obj();
        self.begin_dict(eptr, 1);
        self.print(format!("/Type /Encoding\n/Differences [{first_char}").as_bytes());
        let mut is_notdef;
        if t3_char_procs[first_char] == 0 {
            self.print(b"/.notdef");
            is_notdef = true;
        } else {
            match glyph(first_char) {
                Some(n) => {
                    self.print(b"/");
                    self.print(&n);
                }
                None => self.print(format!("/a{first_char}").as_bytes()),
            }
            is_notdef = false;
        }
        for i in first_char + 1..=last_char {
            if t3_char_procs[i] == 0 {
                if !is_notdef {
                    self.print(format!(" {i}/.notdef").as_bytes());
                    is_notdef = true;
                }
            } else {
                if is_notdef {
                    self.print(format!(" {i}").as_bytes());
                    is_notdef = false;
                }
                match glyph(i) {
                    Some(n) => {
                        self.print(b"/");
                        self.print(&n);
                    }
                    None => self.print(format!("/a{i}").as_bytes()),
                }
            }
        }
        self.print(b"]\n");
        self.end_dict();
        self.begin_dict(cptr, 1);
        for i in first_char..=last_char {
            if t3_char_procs[i] != 0 {
                match glyph(i) {
                    Some(n) => {
                        self.print(b"/");
                        self.print(&n);
                        self.print(format!(" {} 0 R\n", t3_char_procs[i]).as_bytes());
                    }
                    None => self.print(format!("/a{i} {} 0 R\n", t3_char_procs[i]).as_bytes()),
                }
            }
        }
        self.end_dict();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magsteps_as_kpathsea_fixes_them() {
        // (600 dpi's magsteps: 1.2^n, half steps sqrt 1.2)
        assert_eq!(magstep_fix(600, 600), 600);
        assert_eq!(magstep_fix(720, 600), 720);
        assert_eq!(magstep_fix(657, 600), 657);
        assert_eq!(magstep_fix(658, 600), 657);
        assert_eq!(magstep_fix(864, 600), 864);
        assert_eq!(magstep_fix(500, 600), 500);
        assert_eq!(magstep_fix(501, 600), 500);
        assert_eq!(magstep_fix(700, 600), 700);
        assert_eq!(pk_dpi(655_360, 655_360, 600), 600);
        assert_eq!(pk_dpi(786_432, 655_360, 600), 720);
    }
}
