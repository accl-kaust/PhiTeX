//! bmpimage.c, bmpimage.h: BMP images (uncompressed, RLE8 and RLE4; a
//! palette as an `/Indexed` color space).

use crate::ctx::CompatMode;
use crate::obj::STREAM_COMPRESS;
use crate::pdfximage::pdf_ximage_init_image_info;
use crate::prelude::*;

const DIB_FILE_HEADER_SIZE: u32 = 14;
const DIB_CORE_HEADER_SIZE: u32 = 12;
const DIB_INFO_HEADER_SIZE: u32 = 40;
const DIB_INFO_HEADER_SIZE2: u32 = 64;
const DIB_INFO_HEADER_SIZE4: u32 = 108;
const DIB_INFO_HEADER_SIZE5: u32 = 124;

const DIB_COMPRESS_NONE: i32 = 0;
const DIB_COMPRESS_RLE8: i32 = 1;
const DIB_COMPRESS_RLE4: i32 = 2;

/// `struct hdr_info`.
#[derive(Clone, Copy, Default)]
struct HdrInfo {
    offset: u32,
    hsize: u32,
    width: u32,
    height: i32,
    compression: i32,
    /// Bits per pixel.
    bit_count: u16,
    /// Bytes per palette color: 3 for OS2, 4 for Win.
    psize: i32,
    x_pix_per_meter: u32,
    y_pix_per_meter: u32,
}

/// `check_for_bmp`.
pub fn check_for_bmp(fp: &mut MemFile) -> bool {
    fp.rewind();
    let sig = fp.read(2);
    sig.len() == 2 && sig[0] == b'B' && sig[1] == b'M'
}

/// `ULONG_LE` (C's `int` arithmetic).
#[allow(clippy::cast_possible_wrap, reason = "C's int")]
fn ulong_le(b: &[u8]) -> i32 {
    (u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16 | u32::from(b[3]) << 24) as i32
}

/// `USHORT_LE`.
fn ushort_le(b: &[u8]) -> u16 {
    u16::from(b[0]) | u16::from(b[1]) << 8
}

/// `read_header`.
#[allow(clippy::cast_sign_loss, reason = "C's conversions")]
fn read_header(fp: &mut MemFile) -> Option<HdrInfo> {
    let mut hdr = HdrInfo::default();
    let mut buf = [0u8; (DIB_FILE_HEADER_SIZE + DIB_INFO_HEADER_SIZE5 + 4) as usize];
    if fp.read_into(&mut buf[..18]) != 18 {
        warn!("Could not read BMP file header...");
        return None;
    }
    if buf[0] != b'B' || buf[1] != b'M' {
        warn!("File not starting with 'B' 'M'... Not a BMP file?");
        return None;
    }
    // (fsize ignored)
    if ulong_le(&buf[6..]) != 0 {
        warn!("Not a BMP file???");
        return None;
    }
    hdr.offset = ulong_le(&buf[10..]) as u32;
    // info header
    hdr.hsize = ulong_le(&buf[14..]) as u32;
    let n = hdr.hsize.wrapping_sub(4) as usize;
    // (a size C's buffer does not hold is no header it knows)
    if n > buf.len() - 18 || fp.read_into(&mut buf[18..18 + n]) != n {
        warn!("Could not read BMP file header...");
        return None;
    }
    let p = &buf[18..];
    match hdr.hsize {
        DIB_CORE_HEADER_SIZE => {
            hdr.width = u32::from(ushort_le(p));
            hdr.height = i32::from(ushort_le(&p[2..]));
            hdr.x_pix_per_meter = 0; // undefined. FIXME
            hdr.y_pix_per_meter = 0;
            if ushort_le(&p[4..]) != 1 {
                warn!("Unknown bcPlanes value in BMP COREHEADER.");
                return None;
            }
            hdr.bit_count = ushort_le(&p[6..]);
            hdr.compression = DIB_COMPRESS_NONE;
            hdr.psize = 3;
        }
        DIB_INFO_HEADER_SIZE
        | DIB_INFO_HEADER_SIZE2
        | DIB_INFO_HEADER_SIZE4
        | DIB_INFO_HEADER_SIZE5 => {
            hdr.width = ulong_le(p) as u32;
            hdr.height = ulong_le(&p[4..]);
            if ushort_le(&p[8..]) != 1 {
                warn!("Unknown biPlanes value in BMP INFOHEADER.");
                return None;
            }
            hdr.bit_count = ushort_le(&p[10..]);
            hdr.compression = ulong_le(&p[12..]);
            // (biSizeImage ignored)
            hdr.x_pix_per_meter = ulong_le(&p[20..]) as u32;
            hdr.y_pix_per_meter = ulong_le(&p[24..]) as u32;
            hdr.psize = 4;
        }
        _ => {
            warn!("Unknown BMP header type.");
            return None;
        }
    }
    Some(hdr)
}

/// `read_raster_rle8`: false where decoding failed. (Writes C would make
/// past the buffer are dropped.)
fn read_raster_rle8(data: &mut [u8], width: i32, height: i32, fp: &mut MemFile) -> Result<bool> {
    let rowbytes = width;
    data.fill(0);
    let at = |v: i32, h: i32| {
        usize::try_from(i64::from(v) * i64::from(rowbytes) + i64::from(h)).unwrap_or(usize::MAX)
    };
    let mut v = 0;
    let mut eoi = false;
    while v < height && !eoi {
        let mut h = 0;
        let mut eol = false;
        while h < width && !eol {
            let b0 = fp.get_unsigned_byte()?;
            let b1 = fp.get_unsigned_byte()?;
            let p = at(v, h);
            if b0 == 0x00 {
                match b1 {
                    0x00 => eol = true, // EOL
                    0x01 => eoi = true, // EOI
                    0x02 => {
                        h += i32::from(fp.get_unsigned_byte()?);
                        v += i32::from(fp.get_unsigned_byte()?);
                    }
                    _ => {
                        h += i32::from(b1);
                        if h > width {
                            warn!("RLE decode failed...");
                            return Ok(false);
                        }
                        let s = fp.read(usize::from(b1));
                        if s.len() != usize::from(b1) {
                            return Ok(false);
                        }
                        for (k, &c) in s.iter().enumerate() {
                            if let Some(d) = data.get_mut(p.wrapping_add(k)) {
                                *d = c;
                            }
                        }
                        if b1 % 2 != 0 {
                            fp.get_unsigned_byte()?;
                        }
                    }
                }
            } else {
                h += i32::from(b0);
                if h > width {
                    warn!("RLE decode failed...");
                    return Ok(false);
                }
                for k in 0..usize::from(b0) {
                    if let Some(d) = data.get_mut(p.wrapping_add(k)) {
                        *d = b1;
                    }
                }
            }
        }
        // Check for EOL and EOI marker
        if !eol && !eoi {
            let b0 = fp.get_unsigned_byte()?;
            let b1 = fp.get_unsigned_byte()?;
            if b0 != 0x00 {
                warn!("RLE decode failed...");
                return Ok(false);
            } else if b1 == 0x01 {
                eoi = true;
            } else if b1 != 0x00 {
                warn!("RLE decode failed...");
                return Ok(false);
            }
        }
        // next row ...
        v += 1;
    }
    Ok(true)
}

/// `read_raster_rle4`: false where decoding failed.
fn read_raster_rle4(data: &mut [u8], width: i32, height: i32, fp: &mut MemFile) -> Result<bool> {
    let rowbytes = (width + 1) / 2;
    data.fill(0);
    let put = |data: &mut [u8], i: usize, f: &dyn Fn(u8) -> u8| {
        if let Some(d) = data.get_mut(i) {
            *d = f(*d);
        }
    };
    let mut v = 0;
    let mut eoi = false;
    while v < height && !eoi {
        let mut h = 0;
        let mut eol = false;
        while h < width && !eol {
            let b0 = fp.get_unsigned_byte()?;
            let mut b1 = fp.get_unsigned_byte()?;
            let mut p = usize::try_from(i64::from(v) * i64::from(rowbytes) + i64::from(h / 2))
                .unwrap_or(usize::MAX);
            if b0 == 0x00 {
                match b1 {
                    0x00 => eol = true, // EOL
                    0x01 => eoi = true, // EOI
                    0x02 => {
                        h += i32::from(fp.get_unsigned_byte()?);
                        v += i32::from(fp.get_unsigned_byte()?);
                    }
                    _ => {
                        if h + i32::from(b1) > width {
                            warn!("RLE decode failed...");
                            return Ok(false);
                        }
                        let nbytes = (usize::from(b1) + 1) / 2;
                        if h % 2 != 0 {
                            // starting at hi-nib
                            for _ in 0..nbytes {
                                let b = fp.get_unsigned_byte()?;
                                put(data, p, &|d| d | ((b >> 4) & 0x0f));
                                p = p.wrapping_add(1);
                                put(data, p, &|_| (b << 4) & 0xf0);
                            }
                        } else {
                            let s = fp.read(nbytes);
                            if s.len() != nbytes {
                                return Ok(false);
                            }
                            for (k, &c) in s.iter().enumerate() {
                                put(data, p.wrapping_add(k), &|_| c);
                            }
                        }
                        h += i32::from(b1);
                        if nbytes % 2 != 0 {
                            fp.get_unsigned_byte()?;
                        }
                    }
                }
            } else {
                if h + i32::from(b0) > width {
                    warn!("RLE decode failed...");
                    return Ok(false);
                }
                let mut b0 = b0;
                if h % 2 != 0 {
                    put(data, p, &|_| (b1 >> 4) & 0x0f);
                    p = p.wrapping_add(1);
                    b1 = ((b1 << 4) & 0xf0) | ((b1 >> 4) & 0x0f);
                    b0 -= 1;
                    h += 1;
                }
                let nbytes = (usize::from(b0) + 1) / 2;
                for k in 0..nbytes {
                    put(data, p.wrapping_add(k), &|_| b1);
                }
                h += i32::from(b0);
                if h % 2 != 0 {
                    put(data, p.wrapping_add(nbytes).wrapping_sub(1), &|d| d & 0xf0);
                }
            }
        }
        // Check for EOL and EOI marker
        if !eol && !eoi {
            let b0 = fp.get_unsigned_byte()?;
            let b1 = fp.get_unsigned_byte()?;
            if b0 != 0x00 {
                warn!("No EOL/EOI marker. RLE decode failed...");
                return Ok(false);
            } else if b1 == 0x01 {
                eoi = true;
            } else if b1 != 0x00 {
                warn!("No EOL/EOI marker. RLE decode failed...");
                return Ok(false);
            }
        }
        // next row ...
        v += 1;
    }
    Ok(true)
}

impl Dpx {
    /// `get_density`.
    fn bmp_density(&self, hdr: &HdrInfo) -> (f64, f64) {
        if self.conf.compat_mode == CompatMode::Compat {
            (72.0 / 100.0, 72.0 / 100.0)
        } else if hdr.x_pix_per_meter > 0 && hdr.y_pix_per_meter > 0 {
            // 0 for undefined. FIXME
            (
                72.0 / (f64::from(hdr.x_pix_per_meter) * 0.0254),
                72.0 / (f64::from(hdr.y_pix_per_meter) * 0.0254),
            )
        } else {
            (1.0, 1.0)
        }
    }

    /// `bmp_get_bbox`: status, width, height, xdensity, ydensity.
    #[allow(clippy::cast_possible_wrap, reason = "C's int")]
    pub fn bmp_get_bbox(&mut self, fp: &mut MemFile) -> (i32, i32, i32, f64, f64) {
        fp.rewind();
        let r = read_header(fp);
        let hdr = r.unwrap_or_default();
        let (xd, yd) = self.bmp_density(&hdr);
        (
            if r.is_some() { 0 } else { -1 },
            hdr.width as i32,
            hdr.height.wrapping_abs(),
            xd,
            yd,
        )
    }

    /// `bmp_include_image`.
    #[allow(
        clippy::cast_possible_wrap,
        clippy::cast_sign_loss,
        reason = "C's conversions"
    )]
    pub fn bmp_include_image(&mut self, xobj_id: i32, fp: &mut MemFile) -> Result<i32> {
        let mut info = pdf_ximage_init_image_info();
        fp.rewind();
        let Some(hdr) = read_header(fp) else {
            return Ok(-1);
        };
        (info.xdensity, info.ydensity) = self.bmp_density(&hdr);
        info.width = hdr.width as i32;
        info.height = hdr.height;
        let flip = if info.height < 0 {
            info.height = info.height.wrapping_neg();
            false
        } else {
            true
        };
        let num_palette;
        if hdr.bit_count < 24 {
            if hdr.bit_count != 1 && hdr.bit_count != 4 && hdr.bit_count != 8 {
                warn!("Unsupported palette size: {}", hdr.bit_count);
                return Ok(-1);
            }
            // (`unsigned int` arithmetic)
            num_palette = (hdr
                .offset
                .wrapping_sub(hdr.hsize)
                .wrapping_sub(DIB_FILE_HEADER_SIZE)
                / hdr.psize as u32) as i32;
            info.bits_per_component = i32::from(hdr.bit_count);
            info.num_components = 1;
        } else if hdr.bit_count == 24 {
            // full color
            num_palette = 1; // dummy
            info.bits_per_component = 8;
            info.num_components = 3;
        } else {
            warn!("Unkown/Unsupported BMP bitCount value: {}", hdr.bit_count);
            return Ok(-1);
        }
        if info.width == 0 || info.height == 0 || num_palette < 1 {
            warn!(
                "Invalid BMP file: width={}, height={}, #palette={num_palette}",
                info.width, info.height
            );
            return Ok(-1);
        }

        // Start reading raster data
        let stream = self.o.new_stream(STREAM_COMPRESS);
        let stream_dict = self.o.stream_dict(stream);

        // Color space: Indexed or DeviceRGB
        let colorspace = if hdr.bit_count < 24 {
            let psize = hdr.psize as usize;
            let mut palette = Vec::with_capacity(num_palette as usize * 3);
            for _ in 0..num_palette {
                let bgrq = fp.read(psize);
                if bgrq.len() != psize {
                    warn!("Reading file failed...");
                    return Ok(-1);
                }
                // BGR data
                palette.extend_from_slice(&[bgrq[2], bgrq[1], bgrq[0]]);
            }
            let lookup = self.o.new_string(&palette);
            let colorspace = self.o.new_array();
            let n = self.o.new_name(b"Indexed");
            self.o.add_array(colorspace, n);
            let n = self.o.new_name(b"DeviceRGB");
            self.o.add_array(colorspace, n);
            let n = self.o.new_number(f64::from(num_palette - 1));
            self.o.add_array(colorspace, n);
            self.o.add_array(colorspace, lookup);
            colorspace
        } else {
            self.o.new_name(b"DeviceRGB")
        };
        self.o.put(stream_dict, b"ColorSpace", colorspace);

        // Raster data of BMP is four-byte aligned.
        let rowbytes = (info.width.wrapping_mul(i32::from(hdr.bit_count)) + 7) / 8;
        let (urow, uheight) = (rowbytes.max(0) as usize, info.height.max(0) as usize);
        fp.seek_absolute(hdr.offset as usize);
        let mut data;
        if hdr.compression == DIB_COMPRESS_NONE {
            let padding = if rowbytes % 4 != 0 {
                4 - (rowbytes % 4)
            } else {
                0
            } as usize;
            let dib_rowbytes = urow + padding;
            data = vec![0u8; urow * uheight + padding];
            for n in 0..uheight {
                let s = fp.read(dib_rowbytes);
                if s.len() != dib_rowbytes {
                    warn!("Reading BMP raster data failed...");
                    self.o.release(stream);
                    return Ok(-1);
                }
                data[n * urow..n * urow + dib_rowbytes].copy_from_slice(s);
            }
        } else if hdr.compression == DIB_COMPRESS_RLE8 || hdr.compression == DIB_COMPRESS_RLE4 {
            data = vec![0u8; urow * uheight];
            let ok = if hdr.compression == DIB_COMPRESS_RLE8 {
                read_raster_rle8(&mut data, info.width, info.height, fp)?
            } else {
                read_raster_rle4(&mut data, info.width, info.height, fp)?
            };
            if !ok {
                warn!("Reading BMP raster data failed...");
                self.o.release(stream);
                return Ok(-1);
            }
        } else {
            warn!(
                "Unknown/Unsupported compression type for BMP image: {}",
                hdr.compression
            );
            self.o.release(stream);
            return Ok(-1);
        }

        // gbr --> rgb
        if hdr.bit_count == 24 {
            for px in data[..urow * uheight].chunks_exact_mut(3) {
                px.swap(0, 2);
            }
        }

        if flip {
            for n in (0..uheight).rev() {
                self.o.add_stream(stream, &data[n * urow..(n + 1) * urow]);
            }
        } else {
            self.o.add_stream(stream, &data[..urow * uheight]);
        }

        // Predictor is usually not so efficient for indexed images.
        if hdr.bit_count >= 24 && info.bits_per_component >= 8 && info.height > 64 {
            self.o.stream_set_predictor(
                stream,
                15,
                info.width,
                info.bits_per_component,
                info.num_components,
            );
        }
        self.pdf_ximage_set_image(xobj_id, &info, stream)?;

        Ok(0)
    }
}
