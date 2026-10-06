//! pngimage.c, pngimage.h: PNG images.
//!
//! C reads PNGs with libpng (1.6, TeX Live's); here partex_engine::png, the
//! port pdfTeX uses, reads them: [`PngInfo`] holds what libpng's
//! `png_structp` + `png_infop` tell pngimage.c. Its inflater keeps a 32 KiB
//! window whatever the zlib header says (`PNG_MAXIMUM_INFLATE_WINDOW` on).
//! Not in that port, and fatal errors here: `png_set_gamma`'s correction of
//! the samples (a PNG with only a gAMA chunk whose gamma is not about
//! 1/2.2) and `png_set_background` (transparency before PDF 1.4).

use alloc::sync::Arc;

use partex_engine::png;

use crate::ctx::CompatMode;
use crate::fmt::round_acc;
use crate::obj::STREAM_COMPRESS;
use crate::pdfcolor::{PDF_COLORSPACE_TYPE_GRAY, PDF_COLORSPACE_TYPE_RGB};
use crate::pdfximage::pdf_ximage_init_image_info;
use crate::prelude::*;

/// `PNG_DEBUG_STR`.
pub const PNG_DEBUG_STR: &str = "PNG";
/// `PNG_DEBUG`.
pub const PNG_DEBUG: i32 = 3;
/// `DPX_PNG_DEFAULT_GAMMA`.
pub const DPX_PNG_DEFAULT_GAMMA: f64 = 2.2;

/// `PDF_TRANS_TYPE_NONE`.
pub const PDF_TRANS_TYPE_NONE: i32 = 0;
/// `PDF_TRANS_TYPE_BINARY`.
pub const PDF_TRANS_TYPE_BINARY: i32 = 1;
/// `PDF_TRANS_TYPE_ALPHA`.
pub const PDF_TRANS_TYPE_ALPHA: i32 = 2;

/// libpng's `PNG_COLOR_MASK_PALETTE`.
pub const PNG_COLOR_MASK_PALETTE: u8 = 1;
/// libpng's `PNG_COLOR_MASK_COLOR`.
pub const PNG_COLOR_MASK_COLOR: u8 = 2;
/// libpng's `PNG_COLOR_MASK_ALPHA`.
pub const PNG_COLOR_MASK_ALPHA: u8 = 4;
/// libpng's `PNG_COLOR_TYPE_GRAY`.
pub const PNG_COLOR_TYPE_GRAY: u8 = 0;
/// libpng's `PNG_COLOR_TYPE_PALETTE`.
pub const PNG_COLOR_TYPE_PALETTE: u8 = 3;
/// libpng's `PNG_COLOR_TYPE_RGB`.
pub const PNG_COLOR_TYPE_RGB: u8 = 2;
/// libpng's `PNG_COLOR_TYPE_RGB_ALPHA`.
pub const PNG_COLOR_TYPE_RGB_ALPHA: u8 = 6;
/// libpng's `PNG_COLOR_TYPE_GRAY_ALPHA`.
pub const PNG_COLOR_TYPE_GRAY_ALPHA: u8 = 4;

/// libpng's `PNG_sRGB_INTENT_PERCEPTUAL`.
pub const PNG_SRGB_INTENT_PERCEPTUAL: i32 = 0;
/// libpng's `PNG_sRGB_INTENT_RELATIVE`.
pub const PNG_SRGB_INTENT_RELATIVE: i32 = 1;
/// libpng's `PNG_sRGB_INTENT_SATURATION`.
pub const PNG_SRGB_INTENT_SATURATION: i32 = 2;
/// libpng's `PNG_sRGB_INTENT_ABSOLUTE`.
pub const PNG_SRGB_INTENT_ABSOLUTE: i32 = 3;

/// libpng's `PNG_RESOLUTION_METER`.
pub const PNG_RESOLUTION_METER: i32 = 1;
/// libpng's `PNG_ITXT_COMPRESSION_NONE`.
pub const PNG_ITXT_COMPRESSION_NONE: i32 = 1;
/// libpng's `PNG_ITXT_COMPRESSION_zTXt`.
pub const PNG_ITXT_COMPRESSION_ZTXT: i32 = 2;

/// libpng's `png_color_16` (the tRNS color key).
#[derive(Clone, Copy, Debug, Default)]
pub struct PngColor16 {
    pub index: u8,
    pub red: u16,
    pub green: u16,
    pub blue: u16,
    pub gray: u16,
}

/// libpng's `png_text` (tEXt, zTXt, iTXt chunks).
#[derive(Clone, Debug, Default)]
pub struct PngText {
    /// `PNG_TEXT_COMPRESSION_*` / `PNG_ITXT_COMPRESSION_*`.
    pub compression: i32,
    pub key: Vec<u8>,
    /// The text (an uncompressed iTXt's; empty for the others, which
    /// pngimage.c only tells apart by `compression`).
    pub text: Vec<u8>,
    pub itxt_length: usize,
}

/// libpng's `png_structp` + `png_infop` as pngimage.c reads them: what
/// `partex_engine::png::read_info` gives (the header, PLTE, tRNS, gAMA,
/// pHYs, the valid bits), the values of the chunks it judged but does not
/// keep (sRGB, cHRM, iCCP, the text chunks before the image data), and
/// the transformations asked for.
#[derive(Clone, Debug)]
pub struct PngInfo {
    pub info: png::Info,
    /// The file.
    pub data: Arc<[u8]>,
    /// sRGB: the rendering intent (with `INFO_SRGB`).
    pub srgb_intent: i32,
    /// cHRM: white x, y, red x, y, green x, y, blue x, y, as
    /// `png_get_cHRM` gives them (with `INFO_CHRM`).
    pub chrm: [f64; 8],
    /// iCCP: the name and the profile, inflated (with `INFO_ICCP`).
    pub iccp: Option<(Vec<u8>, Vec<u8>)>,
    /// tEXt / zTXt / iTXt before the image data, in file order
    /// (`png_get_text` after `png_read_info`).
    pub text: Vec<PngText>,
    /// `png_set_strip_16` asked.
    pub strip_16: bool,
    /// `png_set_gamma(screen_gamma, file_gamma)` asked.
    pub set_gamma: Option<(f64, f64)>,
    /// After `png_read_update_info`: the transformed color type and depth
    /// (`png_get_color_type`, `png_get_bit_depth`) and `png_get_rowbytes`.
    pub out_color_type: u8,
    pub out_bit_depth: u8,
    pub rowbytes: u32,
}

impl PngInfo {
    /// `png_get_valid`.
    fn valid(&self, flag: u32) -> bool {
        self.info.get_valid(flag) != 0
    }
    /// `png_get_gAMA`: the file gamma, `.00001 * fixed` (`png_float`).
    fn gamma(&self) -> f64 {
        0.00001 * f64::from(self.info.gamma)
    }
    /// The transformations asked, as partex_engine::png takes them.
    fn transforms(&self) -> png::Transforms {
        png::Transforms {
            trns_to_alpha: false,
            strip_alpha: false,
            strip_16: self.strip_16,
        }
    }
}

/// `check_for_png`: 1 if the signature is PNG's.
pub fn check_for_png(fp: &mut MemFile) -> i32 {
    fp.rewind();
    let sig = fp.read(4);
    // `png_sig_cmp(sigbytes, 0, 4)`
    if sig.len() != 4 || sig != [137, 80, 78, 71] {
        return 0;
    }
    1
}

/// The values of the chunks before the first IDAT that libpng keeps and
/// partex_engine::png only judges: the first sRGB's intent, the first
/// cHRM's values, the first iCCP's name and inflated profile, and every
/// text chunk (an iTXt as `png_handle_iTXt` reads it). Whether libpng
/// accepted a chunk is `read_info`'s valid bits; text chunks libpng
/// would reject (a bad keyword, a truncated iTXt) are left out.
fn chunk_values(d: &[u8], png: &mut PngInfo) {
    let mut pos = 8;
    let (mut srgb, mut chrm, mut iccp) = (false, false, false);
    while pos + 8 <= d.len() {
        let length = u32::from_be_bytes([d[pos], d[pos + 1], d[pos + 2], d[pos + 3]]) as usize;
        let name = &d[pos + 4..pos + 8];
        let Some(data) = d.get(pos + 8..pos + 8 + length) else {
            break;
        };
        match name {
            b"IDAT" => break,
            b"sRGB" if !srgb && length == 1 => {
                srgb = true;
                png.srgb_intent = i32::from(data[0]);
            }
            b"cHRM" if !chrm && length == 32 => {
                chrm = true;
                for (i, b) in data.chunks(4).enumerate() {
                    let v = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
                    png.chrm[i] = 0.00001 * f64::from(v as i32);
                }
            }
            b"iCCP" if !iccp => {
                iccp = true;
                if let Some(kw) = data.iter().position(|&c| c == 0)
                    && let Some(z) = data.get(kw + 2..)
                    && let Some(mut profile) = partex_engine::inflate::inflate(z)
                    && profile.len() >= 4
                {
                    let n = u32::from_be_bytes([profile[0], profile[1], profile[2], profile[3]]);
                    profile.truncate(n as usize);
                    png.iccp = Some((data[..kw].to_vec(), profile));
                }
            }
            b"tEXt" | b"zTXt" => {
                if let Some(kw) = data.iter().position(|&c| c == 0)
                    && (1..=79).contains(&kw)
                {
                    png.text.push(PngText {
                        compression: if name == b"tEXt" { -1 } else { 0 },
                        key: data[..kw].to_vec(),
                        text: Vec::new(),
                        itxt_length: 0,
                    });
                }
            }
            b"iTXt" => {
                if let Some(kw) = data.iter().position(|&c| c == 0)
                    && (1..=79).contains(&kw)
                    && kw + 1 + 5 <= length
                {
                    let mut prefix = kw + 1;
                    let compressed = data[prefix];
                    let ok = compressed == 0 || (compressed == 1 && data[prefix + 1] == 0);
                    prefix += 2;
                    let lang = data[prefix..].iter().position(|&c| c == 0);
                    let tkey = lang.and_then(|l| {
                        data[prefix + l + 1..]
                            .iter()
                            .position(|&c| c == 0)
                            .map(|t| prefix + l + 1 + t + 1)
                    });
                    if ok && let Some(text_at) = tkey {
                        let text = if compressed == 0 {
                            data[text_at..].to_vec()
                        } else {
                            Vec::new()
                        };
                        png.text.push(PngText {
                            compression: if compressed != 0 {
                                PNG_ITXT_COMPRESSION_ZTXT
                            } else {
                                PNG_ITXT_COMPRESSION_NONE
                            },
                            key: data[..kw].to_vec(),
                            itxt_length: text.len(),
                            text,
                        });
                    }
                }
            }
            _ => {}
        }
        pos += 12 + length;
    }
}

/// libpng's `png_read_info` (with `png_create_read_struct` and
/// `png_init_io` on the file from its start). pngimage.c sets no
/// `setjmp`: a libpng error aborts the program.
pub fn png_read_info(fp: &mut MemFile) -> Result<PngInfo> {
    fp.rewind();
    let info = match png::read_info(&fp.data) {
        Ok(info) => info,
        Err(e) => fatal!("libpng error: {e}"),
    };
    let mut png = PngInfo {
        out_color_type: info.color_type,
        out_bit_depth: info.bit_depth,
        info,
        data: fp.data.clone(),
        srgb_intent: 0,
        chrm: [0.0; 8],
        iccp: None,
        text: Vec::new(),
        strip_16: false,
        set_gamma: None,
        rowbytes: 0,
    };
    let data = png.data.clone();
    chunk_values(&data, &mut png);
    Ok(png)
}

/// `PNG_ROWBYTES`.
fn png_rowbytes(pixel_bits: u32, width: u32) -> u32 {
    if pixel_bits >= 8 {
        width.wrapping_mul(pixel_bits >> 3)
    } else {
        (width.wrapping_mul(pixel_bits) + 7) >> 3
    }
}

/// `png_gamma_threshold(screen, file)` on `png_set_gamma`'s doubles
/// (`convert_gamma`, then `png_muldiv` and `png_gamma_significant`):
/// whether libpng corrects the samples.
fn png_gamma_correction(screen: f64, file: f64) -> bool {
    let convert = |g: f64| {
        let g = if g > 0.0 && g < 128.0 {
            g * 100_000.0
        } else {
            g
        };
        libm::floor(g + 0.5)
    };
    let (s, f) = (convert(screen), convert(file));
    let r = libm::floor(s * f / 100_000.0 + 0.5);
    if !(-2_147_483_648.0..=2_147_483_647.0).contains(&r) {
        return true;
    }
    !(95_000.0..=105_000.0).contains(&r)
}

/// libpng's `png_read_update_info`: applies the transformations to the
/// reported color type, depth and rowbytes.
pub fn png_read_update_info(png: &mut PngInfo) -> Result<()> {
    if let Some((screen, file)) = png.set_gamma
        && png_gamma_correction(screen, file)
    {
        // (not ported: libpng's gamma correction)
        fatal!("png_set_gamma: gamma correction is not supported (a PNG with only a gAMA chunk)");
    }
    let (color_type, bit_depth) = png::output(&png.info, png.transforms());
    let channels: u32 = match color_type {
        png::COLOR_RGB => 3,
        png::COLOR_GRAY_ALPHA => 2,
        png::COLOR_RGB_ALPHA => 4,
        _ => 1,
    };
    png.out_color_type = color_type;
    png.out_bit_depth = bit_depth;
    png.rowbytes = png_rowbytes(channels * u32::from(bit_depth), png.info.width);
    Ok(())
}

/// `read_image_data` (static): the rows, decoded and transformed, into
/// `dest` (`height * rowbytes` bytes); libpng's `png_read_image`. (The
/// bits past a row's last pixel, which libpng leaves as they were in
/// `dest`, uninitialised memory in C, are 0.)
pub fn read_image_data(
    png: &mut PngInfo,
    dest: &mut [u8],
    height: u32,
    rowbytes: u32,
) -> Result<()> {
    let image = match png::read_image(&png.data, &png.info, png.transforms()) {
        Ok(image) => image,
        Err(e) => fatal!("libpng error: {e}"),
    };
    assert_eq!(image.rowbytes, rowbytes as usize);
    let n = (height as usize) * (rowbytes as usize);
    dest[..n].copy_from_slice(&image.rows[..n]);
    Ok(())
}

/// `INVALID_CHRM_VALUE`.
fn invalid_chrm(c: &[f64; 8]) -> bool {
    let [xw, yw, xr, yr, xg, yg, xb, yb] = *c;
    xw <= 0.0
        || yw < 1.0e-10
        || xr < 0.0
        || yr < 0.0
        || xg < 0.0
        || yg < 0.0
        || xb < 0.0
        || yb < 0.0
}

impl Dpx {
    /// `check_transparency` (static): a `PDF_TRANS_TYPE_*`.
    pub fn check_transparency(&mut self, png: &mut PngInfo) -> Result<i32> {
        let color_type = png.info.color_type;

        /*
         * First we set trans_type to appropriate value for PNG image.
         */
        let mut trans_type =
            if color_type == PNG_COLOR_TYPE_RGB_ALPHA || color_type == PNG_COLOR_TYPE_GRAY_ALPHA {
                PDF_TRANS_TYPE_ALPHA
            } else if png.valid(png::INFO_TRNS) {
                /* Have valid tRNS chunk. */
                match color_type {
                    PNG_COLOR_TYPE_PALETTE => {
                        /* Use color-key mask if possible. */
                        let mut t = PDF_TRANS_TYPE_BINARY;
                        let trans = &png.info.trans_alpha;
                        let mut num_trans = i32::from(png.info.num_trans);
                        while num_trans > 0 {
                            num_trans -= 1;
                            let a = trans[num_trans as usize];
                            if a != 0x00 && a != 0xff {
                                /* This seems not binary transparency */
                                t = PDF_TRANS_TYPE_ALPHA;
                                break;
                            }
                        }
                        t
                    }
                    /* RGB or GRAY, single color specified by trans_values is transparent. */
                    PNG_COLOR_TYPE_GRAY | PNG_COLOR_TYPE_RGB => PDF_TRANS_TYPE_BINARY,
                    /* Else tRNS silently ignored. */
                    _ => PDF_TRANS_TYPE_NONE,
                }
            } else {
                /* no transparency */
                PDF_TRANS_TYPE_NONE
            };

        /*
         * Now we check PDF version.
         */
        if (self.o.check_version(1, 3) < 0 && trans_type != PDF_TRANS_TYPE_NONE)
            || (self.o.check_version(1, 4) < 0 && trans_type == PDF_TRANS_TYPE_ALPHA)
        {
            /* png_set_background(white): composited with a white background */
            // (not ported: libpng's background compositing)
            fatal!("png_set_background: transparency before PDF 1.3/1.4 is not supported");
            #[allow(unreachable_code)]
            {
                trans_type = PDF_TRANS_TYPE_NONE;
            }
        }

        Ok(trans_type)
    }
    /// `png_include_image`: fills XObject `xobj_id`; 0 or -1.
    pub fn png_include_image(&mut self, xobj_id: i32, fp: &mut MemFile) -> Result<i32> {
        let mut info = pdf_ximage_init_image_info();

        /* Read PNG info-header and get some info. */
        let mut png = png_read_info(fp)?;
        let color_type = png.info.color_type;
        let width = png.info.width;
        let height = png.info.height;
        let mut bpc = png.info.bit_depth;

        /* Ask libpng to convert down to 8-bpc. */
        if bpc > 8 && self.o.check_version(1, 5) < 0 {
            warn!("{PNG_DEBUG_STR}: 16-bpc PNG requires PDF version 1.5.");
            png.strip_16 = true;
            bpc = 8;
        }
        /* Ask libpng to gamma-correct.
         * It is wrong to assume screen gamma value 2.2 but...
         * We do gamma correction here only when uncalibrated color space is used.
         */
        if !png.valid(png::INFO_ICCP)
            && !png.valid(png::INFO_SRGB)
            && !png.valid(png::INFO_CHRM)
            && png.valid(png::INFO_GAMA)
        {
            let g = png.gamma();
            png.set_gamma = Some((2.2, g));
        }

        let trans_type = self.check_transparency(&mut png)?;
        /* check_transparency() does not do updata_info() */
        png_read_update_info(&mut png)?;
        let mut rowbytes = png.rowbytes;

        /* Values listed below will not be modified in the remaining process. */
        info.width = width as i32;
        info.height = height as i32;
        info.bits_per_component = i32::from(bpc);

        if self.conf.compat_mode == CompatMode::Compat {
            info.xdensity = 72.0 / 100.0;
            info.ydensity = 72.0 / 100.0;
        } else {
            let (xppm, yppm) = pixels_per_meter(&png);

            if xppm > 0 {
                info.xdensity = 72.0 / 0.0254 / f64::from(xppm);
            }
            if yppm > 0 {
                info.ydensity = 72.0 / 0.0254 / f64::from(yppm);
            }
        }

        let stream = self.o.new_stream(STREAM_COMPRESS);
        let stream_dict = self.o.stream_dict(stream)?;

        let mut stream_data = vec![0u8; rowbytes.wrapping_mul(height) as usize];
        read_image_data(&mut png, &mut stream_data, height, rowbytes)?;

        /* Non-NULL intent means there is valid sRGB chunk. */
        let intent = self.get_rendering_intent(&png);
        if let Some(intent) = intent {
            self.o.put(stream_dict, b"Intent", intent)?;
        }

        let mut colorspace = None;
        let mut mask = None;
        match color_type {
            PNG_COLOR_TYPE_PALETTE => {
                colorspace = self.create_cspace_Indexed(&png)?;

                match trans_type {
                    PDF_TRANS_TYPE_BINARY => {
                        /* Color-key masking */
                        mask = self.create_ckey_mask(&png)?;
                    }
                    PDF_TRANS_TYPE_ALPHA => {
                        /* Soft mask */
                        mask = self.create_soft_mask(&png, &stream_data, width, height)?;
                    }
                    _ => {
                        /* Nothing to be done here.
                         * No tRNS chunk or image already composited with background color.
                         */
                    }
                }
                info.num_components = 1;
            }
            PNG_COLOR_TYPE_RGB | PNG_COLOR_TYPE_RGB_ALPHA => {
                colorspace = if png.valid(png::INFO_ICCP) {
                    self.create_cspace_ICCBased(&png)?
                } else if intent.is_some() {
                    self.create_cspace_sRGB(&png)?
                } else {
                    self.create_cspace_CalRGB(&png)?
                };
                if colorspace.is_none() {
                    colorspace = Some(self.o.new_name(b"DeviceRGB"));
                }

                mask = match trans_type {
                    PDF_TRANS_TYPE_BINARY => self.create_ckey_mask(&png)?,
                    /* rowbytes changes 4 to 3 at here */
                    PDF_TRANS_TYPE_ALPHA => {
                        self.strip_soft_mask(&png, &mut stream_data, &mut rowbytes, width, height)?
                    }
                    _ => None,
                };
                info.num_components = 3;
            }
            PNG_COLOR_TYPE_GRAY | PNG_COLOR_TYPE_GRAY_ALPHA => {
                colorspace = if png.valid(png::INFO_ICCP) {
                    self.create_cspace_ICCBased(&png)?
                } else if intent.is_some() {
                    self.create_cspace_sRGB(&png)?
                } else {
                    self.create_cspace_CalGray(&png)?
                };
                if colorspace.is_none() {
                    colorspace = Some(self.o.new_name(b"DeviceGray"));
                }

                mask = match trans_type {
                    PDF_TRANS_TYPE_BINARY => self.create_ckey_mask(&png)?,
                    PDF_TRANS_TYPE_ALPHA => {
                        self.strip_soft_mask(&png, &mut stream_data, &mut rowbytes, width, height)?
                    }
                    _ => None,
                };
                info.num_components = 1;
            }
            _ => {
                warn!("{PNG_DEBUG_STR}: Unknown PNG colortype {color_type}.");
            }
        }
        self.o.put_opt(stream_dict, b"ColorSpace", colorspace)?;

        let n = rowbytes.wrapping_mul(height) as usize;
        self.o.add_stream(stream, &stream_data[..n])?;
        drop(stream_data);

        if let Some(mask) = mask {
            if trans_type == PDF_TRANS_TYPE_BINARY {
                self.o.put(stream_dict, b"Mask", mask)?;
            } else if trans_type == PDF_TRANS_TYPE_ALPHA {
                if info.bits_per_component >= 8 && info.width > 64 {
                    self.o
                        .stream_set_predictor(mask, 2, info.width, info.bits_per_component, 1)?;
                }
                let r = self.o.ref_obj(mask)?;
                self.o.put(stream_dict, b"SMask", r)?;
                self.o.release(mask)?;
            } else {
                warn!("{PNG_DEBUG_STR}: Unknown transparency type...???");
                self.o.release(mask)?;
            }
        }

        /* Finally read XMP Metadata
         * See, XMP Specification Part 3, Storage in Files
         * http://www.adobe.com/jp/devnet/xmp.html
         *
         * We require libpng version >= 1.6.14 since prior versions
         * of libpng had a bug that incorrectly treat the compression
         * flag of iTxt chunks.
         */
        if self.o.check_version(1, 4) >= 0 {
            let mut have_xmp = false;

            for t in &png.text {
                // `memcmp(key, "XML:com.adobe.xmp", 17)`: the key and its NUL
                let mut key = t.key.clone();
                key.push(0);
                if key.len() >= 17 && key[..17] == *b"XML:com.adobe.xmp" {
                    /* XMP found */
                    if t.compression != PNG_ITXT_COMPRESSION_NONE || t.itxt_length == 0 {
                        warn!("{PNG_DEBUG_STR}: Invalid value(s) in iTXt chunk for XMP Metadata.");
                    } else if have_xmp {
                        warn!(
                            "{PNG_DEBUG_STR}: Multiple XMP Metadata. Don't know how to treat it."
                        );
                    } else {
                        /* We compress XMP metadata for included images here.
                         * It is not recommended to compress XMP metadata for PDF documents but
                         * we compress XMP metadata for included images here to avoid confusing
                         * application programs that only want PDF document global XMP metadata
                         * and scan for that.
                         */
                        let xmp_stream = self.o.new_stream(STREAM_COMPRESS);
                        let xmp_stream_dict = self.o.stream_dict(xmp_stream)?;
                        self.o.put_name(xmp_stream_dict, b"Type", b"Metadata")?;
                        self.o.put_name(xmp_stream_dict, b"Subtype", b"XML")?;
                        self.o.add_stream(xmp_stream, &t.text[..t.itxt_length])?;
                        let r = self.o.ref_obj(xmp_stream)?;
                        self.o.put(stream_dict, b"Metadata", r)?;
                        self.o.release(xmp_stream)?;
                        have_xmp = true;
                    }
                }
            }
        }

        /* png_read_end(): the chunks after the image data are not read
         * (partex_engine::png reads up to the image data's end) */

        if color_type != PNG_COLOR_TYPE_PALETTE && info.bits_per_component >= 8 && info.height > 64
        {
            self.o.stream_set_predictor(
                stream,
                15,
                info.width,
                info.bits_per_component,
                info.num_components,
            )?;
        }
        self.pdf_ximage_set_image(xobj_id, &info, stream)?;

        Ok(0)
    }
    /// `png_get_bbox`: status, width, height, xdensity, ydensity.
    pub fn png_get_bbox(&mut self, fp: &mut MemFile) -> Result<(i32, u32, u32, f64, f64)> {
        /* Read PNG info-header and get some info. */
        let png = png_read_info(fp)?;
        let width = png.info.width;
        let height = png.info.height;

        let (xdensity, ydensity) = if self.conf.compat_mode == CompatMode::Compat {
            (72.0 / 100.0, 72.0 / 100.0)
        } else {
            let (xppm, yppm) = pixels_per_meter(&png);

            (
                if xppm != 0 {
                    72.0 / 0.0254 / f64::from(xppm)
                } else {
                    1.0
                },
                if yppm != 0 {
                    72.0 / 0.0254 / f64::from(yppm)
                } else {
                    1.0
                },
            )
        };

        Ok((0, width, height, xdensity, ydensity))
    }
    /// `create_cspace_Indexed` (static).
    #[allow(non_snake_case)]
    pub fn create_cspace_Indexed(&mut self, png: &PngInfo) -> Result<Option<Obj>> {
        if !png.valid(png::INFO_PLTE) || png.info.palette.is_empty() {
            warn!("{PNG_DEBUG_STR}: PNG does not have valid PLTE chunk.");
            return Ok(None);
        }
        let plte = &png.info.palette;
        let num_plte = plte.len();

        /* Order is important. */
        let colorspace = self.o.new_array();
        let n = self.o.new_name(b"Indexed");
        self.o.add_array(colorspace, n)?;

        let base = if png.valid(png::INFO_ICCP) {
            self.create_cspace_ICCBased(png)?
        } else if png.valid(png::INFO_SRGB) {
            self.create_cspace_sRGB(png)?
        } else {
            self.create_cspace_CalRGB(png)?
        };

        let base = match base {
            Some(b) => b,
            None => self.o.new_name(b"DeviceRGB"),
        };

        self.o.add_array(colorspace, base)?;
        let n = self.o.new_number((num_plte as i32 - 1) as f64);
        self.o.add_array(colorspace, n)?;
        let data: Vec<u8> = plte.iter().flat_map(|c| c.iter().copied()).collect();
        let lookup = self.o.new_string(&data);
        self.o.add_array(colorspace, lookup)?;

        Ok(Some(colorspace))
    }
    /// `create_cspace_CalRGB` (static).
    #[allow(non_snake_case)]
    pub fn create_cspace_CalRGB(&mut self, png: &PngInfo) -> Result<Option<Obj>> {
        self.png_create_cspace_cal(png, PNG_COLOR_TYPE_RGB, b"CalRGB")
    }
    /// `create_cspace_CalGray` (static).
    #[allow(non_snake_case)]
    pub fn create_cspace_CalGray(&mut self, png: &PngInfo) -> Result<Option<Obj>> {
        self.png_create_cspace_cal(png, PNG_COLOR_TYPE_GRAY, b"CalGray")
    }
    /// The body of `create_cspace_CalRGB` and `create_cspace_CalGray`.
    fn png_create_cspace_cal(
        &mut self,
        png: &PngInfo,
        color_type: u8,
        name: &[u8],
    ) -> Result<Option<Obj>> {
        if !png.valid(png::INFO_CHRM) {
            return Ok(None);
        }

        let c = png.chrm;
        if invalid_chrm(&c) {
            warn!("{PNG_DEBUG_STR}: Invalid cHRM chunk parameters found.");
            return Ok(None);
        }

        let g = if png.valid(png::INFO_GAMA) {
            let g = png.gamma();
            if g < 1.0e-2 {
                warn!("{PNG_DEBUG_STR}: Unusual Gamma value: 1.0 / {g}");
                return Ok(None);
            }
            1.0 / g /* Gamma is inverted. */
        } else {
            DPX_PNG_DEFAULT_GAMMA
        };

        let cal_param = some!(self.make_param_Cal(
            color_type, g, c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7],
        )?);

        let colorspace = self.o.new_array();
        let n = self.o.new_name(name);
        self.o.add_array(colorspace, n)?;
        self.o.add_array(colorspace, cal_param)?;

        Ok(Some(colorspace))
    }
    /// `make_param_Cal` (static).
    #[allow(non_snake_case)]
    pub fn make_param_Cal(
        &mut self,
        color_type: u8,
        g: f64,
        xw: f64,
        yw: f64,
        xr: f64,
        yr: f64,
        xg: f64,
        yg: f64,
        xb: f64,
        yb: f64,
    ) -> Result<Option<Obj>> {
        /*
         * TODO: Check validity
         *
         * Conversion found in
         *
         *  com.sixlegs.image.png - Java package to read and display PNG images
         *  Copyright (C) 1998, 1999, 2001 Chris Nokleberg
         */
        /* WhitePoint */
        let zw = 1.0 - (xw + yw);
        let zr = 1.0 - (xr + yr);
        let zg = 1.0 - (xg + yg);
        let zb = 1.0 - (xb + yb);
        let x_w = xw / yw;
        let y_w = 1.0;
        let z_w = zw / yw;

        /* Matrix */
        let det = xr * (yg * zb - zg * yb) - xg * (yr * zb - zr * yb) + xb * (yr * zg - zr * yg);
        if det.abs() < 1.0e-10 {
            warn!("Non invertible matrix: Maybe invalid value(s) specified in cHRM chunk.");
            return Ok(None);
        }
        let fr = (x_w * (yg * zb - zg * yb) - xg * (zb - z_w * yb) + xb * (zg - z_w * yg)) / det;
        let fg = (xr * (zb - z_w * yb) - x_w * (yr * zb - zr * yb) + xb * (yr * z_w - zr)) / det;
        let fb = (xr * (yg * z_w - zg) - xg * (yr * z_w - zr) + x_w * (yr * zg - zr * yg)) / det;
        let (x_r, y_r, z_r) = (fr * xr, fr * yr, fr * zr);
        let (x_g, y_g, z_g) = (fg * xg, fg * yg, fg * zg);
        let (x_b, y_b, z_b) = (fb * xb, fb * yb, fb * zb);

        if g < 1.0e-2 {
            warn!("Unusual Gamma specified: 1.0 / {g}");
            return Ok(None);
        }

        let cal_param = self.o.new_dict();

        let round = |v: f64| round_acc(v, 0.00001);
        /* White point is always required. */
        let white_point = self.o.new_array();
        for v in [x_w, y_w, z_w] {
            let n = self.o.new_number(round(v));
            self.o.add_array(white_point, n)?;
        }
        self.o.put(cal_param, b"WhitePoint", white_point)?;

        /* Matrix - default: Identity */
        if color_type & PNG_COLOR_MASK_COLOR != 0 {
            if g != 1.0 {
                let dev_gamma = self.o.new_array();
                for _ in 0..3 {
                    let n = self.o.new_number(round(g));
                    self.o.add_array(dev_gamma, n)?;
                }
                self.o.put(cal_param, b"Gamma", dev_gamma)?;
            }

            let matrix = self.o.new_array();
            for v in [x_r, y_r, z_r, x_g, y_g, z_g, x_b, y_b, z_b] {
                let n = self.o.new_number(round(v));
                self.o.add_array(matrix, n)?;
            }
            self.o.put(cal_param, b"Matrix", matrix)?;
        } else {
            /* Gray */
            if g != 1.0 {
                self.o.put_number(cal_param, b"Gamma", round(g))?;
            }
        }

        Ok(Some(cal_param))
    }
    /// `create_cspace_sRGB` (static).
    #[allow(non_snake_case)]
    pub fn create_cspace_sRGB(&mut self, png: &PngInfo) -> Result<Option<Obj>> {
        let color_type = png.info.color_type;

        /* Parameters taken from PNG spec. section 4.2.2.3. */
        let cal_param = some!(self.make_param_Cal(
            color_type, 2.2, 0.3127, 0.329, 0.64, 0.33, 0.3, 0.6, 0.15, 0.06,
        )?);

        let colorspace = self.o.new_array();

        match color_type {
            PNG_COLOR_TYPE_RGB | PNG_COLOR_TYPE_RGB_ALPHA | PNG_COLOR_TYPE_PALETTE => {
                let n = self.o.new_name(b"CalRGB");
                self.o.add_array(colorspace, n)?;
            }
            PNG_COLOR_TYPE_GRAY | PNG_COLOR_TYPE_GRAY_ALPHA => {
                let n = self.o.new_name(b"CalGray");
                self.o.add_array(colorspace, n)?;
            }
            _ => {}
        }
        self.o.add_array(colorspace, cal_param)?;

        Ok(Some(colorspace))
    }
    /// `get_rendering_intent` (static).
    pub fn get_rendering_intent(&mut self, png: &PngInfo) -> Option<Obj> {
        if !png.valid(png::INFO_SRGB) {
            return None;
        }
        match png.srgb_intent {
            PNG_SRGB_INTENT_SATURATION => Some(self.o.new_name(b"Saturation")),
            PNG_SRGB_INTENT_PERCEPTUAL => Some(self.o.new_name(b"Perceptual")),
            PNG_SRGB_INTENT_ABSOLUTE => Some(self.o.new_name(b"AbsoluteColorimetric")),
            PNG_SRGB_INTENT_RELATIVE => Some(self.o.new_name(b"RelativeColorimetric")),
            i => {
                warn!("{PNG_DEBUG_STR}: Invalid value in PNG sRGB chunk: {i}");
                None
            }
        }
    }
    /// `create_cspace_ICCBased` (static).
    #[allow(non_snake_case)]
    pub fn create_cspace_ICCBased(&mut self, png: &PngInfo) -> Result<Option<Obj>> {
        if !png.valid(png::INFO_ICCP) {
            return Ok(None);
        }
        let (name, profile) = some!(png.iccp.as_ref());

        let color_type = png.info.color_type;

        let colortype = if color_type & PNG_COLOR_MASK_COLOR != 0 {
            PDF_COLORSPACE_TYPE_RGB
        } else {
            PDF_COLORSPACE_TYPE_GRAY
        };

        if self.iccp_check_colorspace(colortype, profile) < 0 {
            Ok(None)
        } else {
            let csp_id = self.iccp_load_profile(Some(name), profile)?;
            if csp_id < 0 {
                Ok(None)
            } else {
                Ok(Some(self.pdf_get_colorspace_reference(csp_id)?))
            }
        }

        /* Rendering intent ... */
    }
    /// `create_ckey_mask` (static).
    pub fn create_ckey_mask(&mut self, png: &PngInfo) -> Result<Option<Obj>> {
        if !png.valid(png::INFO_TRNS) {
            warn!("{PNG_DEBUG_STR}: PNG does not have valid tRNS chunk!");
            return Ok(None);
        }

        let colorkeys = self.o.new_array();
        let color_type = png.info.color_type;
        let colors = png.info.trans_color;

        let add = |dpx: &mut Dpx, v: f64| -> Result<()> {
            let n = dpx.o.new_number(v);
            dpx.o.add_array(colorkeys, n)?;
            Ok(())
        };
        match color_type {
            PNG_COLOR_TYPE_PALETTE => {
                let trans = &png.info.trans_alpha;
                for i in 0..usize::from(png.info.num_trans) {
                    if trans[i] == 0x00 {
                        add(self, i as f64)?;
                        add(self, i as f64)?;
                    } else if trans[i] != 0xff {
                        warn!("{PNG_DEBUG_STR}: You found a bug in pngimage.c.");
                    }
                }
            }
            PNG_COLOR_TYPE_RGB => {
                for c in [colors[1], colors[2], colors[3]] {
                    add(self, f64::from(c))?;
                    add(self, f64::from(c))?;
                }
            }
            PNG_COLOR_TYPE_GRAY => {
                add(self, f64::from(colors[0]))?;
                add(self, f64::from(colors[0]))?;
            }
            _ => {
                warn!("{PNG_DEBUG_STR}: You found a bug in pngimage.c.");
                self.o.release(colorkeys)?;
                return Ok(None);
            }
        }

        Ok(Some(colorkeys))
    }
    /// `create_soft_mask` (static): for palette images. (As in C, the
    /// samples are read as one stream of bits, rows' padding included.)
    pub fn create_soft_mask(
        &mut self,
        png: &PngInfo,
        image_data: &[u8],
        width: u32,
        height: u32,
    ) -> Result<Option<Obj>> {
        if !png.valid(png::INFO_TRNS) {
            warn!("{PNG_DEBUG_STR}: PNG does not have valid tRNS chunk but tRNS is requested.");
            return Ok(None);
        }
        let trans = &png.info.trans_alpha;
        let num_trans = u32::from(png.info.num_trans);
        let bpc = u32::from(png.out_bit_depth);
        let mask: u32 = 0xff >> (8 - bpc);
        let shift: u32 = 8 - bpc;

        let smask = self.o.new_stream(STREAM_COMPRESS);
        let dict = self.o.stream_dict(smask)?;
        let n = width.wrapping_mul(height);
        let mut smask_data = vec![0u8; n as usize];
        self.o.put_name(dict, b"Type", b"XObject")?;
        self.o.put_name(dict, b"Subtype", b"Image")?;
        self.o.put_number(dict, b"Width", f64::from(width))?;
        self.o.put_number(dict, b"Height", f64::from(height))?;
        self.o.put_name(dict, b"ColorSpace", b"DeviceGray")?;
        self.o.put_number(dict, b"BitsPerComponent", 8.0)?;
        for i in 0..n {
            /* data is packed for 1/2/4 bpc formats, msb first */
            let bi = bpc.wrapping_mul(i);
            let idx =
                (u32::from(image_data[(bi / 8) as usize]) >> shift.wrapping_sub(bi % 8)) & mask;
            smask_data[i as usize] = if idx < num_trans {
                trans[idx as usize]
            } else {
                0xff
            };
        }
        self.o.add_stream(smask, &smask_data)?;

        Ok(Some(smask))
    }
    /// `strip_soft_mask` (static): the alpha channel as an SMask; strips it
    /// from `image_data` in place and updates `rowbytes`.
    pub fn strip_soft_mask(
        &mut self,
        png: &PngInfo,
        image_data: &mut Vec<u8>,
        rowbytes: &mut u32,
        width: u32,
        height: u32,
    ) -> Result<Option<Obj>> {
        let color_type = png.out_color_type;
        let bpc = u32::from(png.out_bit_depth);
        let bps: u32 = if color_type & PNG_COLOR_MASK_COLOR != 0 {
            if bpc == 8 { 4 } else { 8 }
        } else if bpc == 8 {
            2
        } else {
            4
        };
        if u64::from(*rowbytes) != u64::from(bps) * u64::from(width) {
            /* Something wrong */
            warn!("{PNG_DEBUG_STR}: Inconsistent rowbytes value.");
            return Ok(None);
        }

        let smask = self.o.new_stream(STREAM_COMPRESS);
        let dict = self.o.stream_dict(smask)?;
        self.o.put_name(dict, b"Type", b"XObject")?;
        self.o.put_name(dict, b"Subtype", b"Image")?;
        self.o.put_number(dict, b"Width", f64::from(width))?;
        self.o.put_number(dict, b"Height", f64::from(height))?;
        self.o.put_name(dict, b"ColorSpace", b"DeviceGray")?;
        self.o
            .put_number(dict, b"BitsPerComponent", f64::from(bpc))?;

        let n = (width as usize) * (height as usize);
        let mut smask_data = vec![0u8; (bpc as usize / 8) * n];
        let d = image_data;

        match color_type {
            PNG_COLOR_TYPE_RGB_ALPHA => {
                if bpc == 8 {
                    for i in 0..n {
                        d.copy_within(4 * i..4 * i + 3, 3 * i);
                        smask_data[i] = d[4 * i + 3];
                    }
                    *rowbytes = 3 * width;
                } else {
                    for i in 0..n {
                        d.copy_within(8 * i..8 * i + 6, 6 * i);
                        smask_data[2 * i] = d[8 * i + 6];
                        smask_data[2 * i + 1] = d[8 * i + 7];
                    }
                    *rowbytes = 6 * width;
                }
            }
            PNG_COLOR_TYPE_GRAY_ALPHA => {
                if bpc == 8 {
                    for i in 0..n {
                        d[i] = d[2 * i];
                        smask_data[i] = d[2 * i + 1];
                    }
                    *rowbytes = width;
                } else {
                    for i in 0..n {
                        d[2 * i] = d[4 * i];
                        d[2 * i + 1] = d[4 * i + 1];
                        smask_data[2 * i] = d[4 * i + 2];
                        smask_data[2 * i + 1] = d[4 * i + 3];
                    }
                    *rowbytes = 2 * width;
                }
            }
            _ => {
                warn!("You found a bug in pngimage.c!");
                self.o.release(smask)?;
                return Ok(None);
            }
        }

        self.o.add_stream(smask, &smask_data)?;

        Ok(Some(smask))
    }
}

/// `png_get_x_pixels_per_meter`, `png_get_y_pixels_per_meter`: with a
/// valid pHYs in meters, else 0.
fn pixels_per_meter(png: &PngInfo) -> (u32, u32) {
    match png.info.phys {
        Some((x, y, unit))
            if png.valid(png::INFO_PHYS) && i32::from(unit) == PNG_RESOLUTION_METER =>
        {
            (x, y)
        }
        _ => (0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gamma_threshold() {
        // 1/2.2 as PNG files write it: 2.2 * 0.45455 is 1
        assert!(!png_gamma_correction(2.2, 0.45455));
        assert!(!png_gamma_correction(2.2, 0.4545));
        // linear data: corrected
        assert!(png_gamma_correction(2.2, 1.0));
    }

    #[test]
    fn rowbytes() {
        assert_eq!(png_rowbytes(1, 9), 2);
        assert_eq!(png_rowbytes(4, 3), 2);
        assert_eq!(png_rowbytes(24, 5), 15);
        assert_eq!(png_rowbytes(64, 2), 16);
    }

    fn file(d: &[u8]) -> MemFile {
        MemFile::new(Arc::from(d), b"t.png")
    }

    fn rows(png: &mut PngInfo) -> Vec<u8> {
        png_read_update_info(png).unwrap();
        let (h, rb) = (png.info.height, png.rowbytes);
        let mut d = vec![0; (h * rb) as usize];
        read_image_data(png, &mut d, h, rb).unwrap();
        d
    }

    #[test]
    fn e2e_files() {
        let mut srgb = png_read_info(&mut file(include_bytes!(
            "../../../tests/e2e/png-rgb8srgb.png"
        )))
        .unwrap();
        assert!(srgb.valid(png::INFO_SRGB));
        assert!((0..=3).contains(&srgb.srgb_intent));
        let a = rows(&mut srgb);
        assert_eq!(srgb.rowbytes, 37 * 3);
        let mut plain =
            png_read_info(&mut file(include_bytes!("../../../tests/e2e/png-rgb8.png"))).unwrap();
        assert!(!plain.valid(png::INFO_SRGB));
        assert_eq!(rows(&mut plain), a);

        let mut pal = png_read_info(&mut file(include_bytes!(
            "../../../tests/e2e/png-pal4trns.png"
        )))
        .unwrap();
        assert!(pal.valid(png::INFO_TRNS) && pal.info.num_trans > 0);
        rows(&mut pal);
        assert_eq!((pal.out_bit_depth, pal.rowbytes), (4, 19));

        let data = include_bytes!("../../../tests/e2e/png-gray16.png");
        let mut g16 = png_read_info(&mut file(data)).unwrap();
        let wide = rows(&mut g16);
        assert_eq!(g16.rowbytes, 74);
        let mut g8 = png_read_info(&mut file(data)).unwrap();
        g8.strip_16 = true;
        let narrow = rows(&mut g8);
        assert_eq!(g8.rowbytes, 37);
        // png_set_strip_16: the high bytes
        assert!(
            narrow
                .iter()
                .zip(wide.iter().step_by(2))
                .all(|(a, b)| a == b)
        );
    }

    #[test]
    fn signature() {
        let mut fp = MemFile::new(Arc::from(&b"\x89PNG\r\n\x1a\n"[..]), b"t.png");
        assert_eq!(check_for_png(&mut fp), 1);
        let mut fp = MemFile::new(Arc::from(&b"\x89PNx"[..]), b"t.png");
        assert_eq!(check_for_png(&mut fp), 0);
    }
}
