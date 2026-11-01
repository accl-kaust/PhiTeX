//! jpegimage.c, jpegimage.h: JPEG images (copied as DCTDecode streams).

use crate::ctx::CompatMode;
use crate::obj::STREAM_COMPRESS;
use crate::pdfcolor::{
    PDF_COLORSPACE_TYPE_CMYK, PDF_COLORSPACE_TYPE_GRAY, PDF_COLORSPACE_TYPE_RGB,
};
use crate::pdfximage::pdf_ximage_init_image_info;
use crate::prelude::*;

/// `JPEG_DEBUG_STR`.
pub const JPEG_DEBUG_STR: &str = "JPEG";
/// `JPEG_DEBUG`.
pub const JPEG_DEBUG: i32 = 3;

/// `JPEG_marker`: a marker byte; `JPEG_get_marker` gives -1 at the end.
pub type JpegMarker = i32;

pub const JM_SOF0: JpegMarker = 0xc0;
pub const JM_SOF1: JpegMarker = 0xc1;
pub const JM_SOF2: JpegMarker = 0xc2;
pub const JM_SOF3: JpegMarker = 0xc3;
pub const JM_SOF5: JpegMarker = 0xc5;
pub const JM_DHT: JpegMarker = 0xc4;
pub const JM_SOF6: JpegMarker = 0xc6;
pub const JM_SOF7: JpegMarker = 0xc7;
pub const JM_SOF9: JpegMarker = 0xc9;
pub const JM_SOF10: JpegMarker = 0xca;
pub const JM_SOF11: JpegMarker = 0xcb;
pub const JM_DAC: JpegMarker = 0xcc;
pub const JM_SOF13: JpegMarker = 0xcd;
pub const JM_SOF14: JpegMarker = 0xce;
pub const JM_SOF15: JpegMarker = 0xcf;
pub const JM_RST0: JpegMarker = 0xd0;
pub const JM_RST1: JpegMarker = 0xd1;
pub const JM_RST2: JpegMarker = 0xd2;
pub const JM_RST3: JpegMarker = 0xd3;
pub const JM_RST4: JpegMarker = 0xd4;
pub const JM_RST5: JpegMarker = 0xd5;
pub const JM_RST6: JpegMarker = 0xd6;
pub const JM_RST7: JpegMarker = 0xd7;
pub const JM_SOI: JpegMarker = 0xd8;
pub const JM_EOI: JpegMarker = 0xd9;
pub const JM_SOS: JpegMarker = 0xda;
pub const JM_DQT: JpegMarker = 0xdb;
pub const JM_DNL: JpegMarker = 0xdc;
pub const JM_DRI: JpegMarker = 0xdd;
pub const JM_DHP: JpegMarker = 0xde;
pub const JM_EXP: JpegMarker = 0xdf;
pub const JM_APP0: JpegMarker = 0xe0;
pub const JM_APP1: JpegMarker = 0xe1;
pub const JM_APP2: JpegMarker = 0xe2;
pub const JM_APP14: JpegMarker = 0xee;
pub const JM_APP15: JpegMarker = 0xef;
pub const JM_COM: JpegMarker = 0xfe;

/// `MAX_COUNT`: chunks numbered for `skipbits`.
pub const MAX_COUNT: usize = 1024;

/// `HAVE_APPn_JFIF`.
pub const HAVE_APPN_JFIF: i32 = 1 << 0;
/// `HAVE_APPn_ADOBE`.
pub const HAVE_APPN_ADOBE: i32 = 1 << 1;
/// `HAVE_APPn_ICC`.
pub const HAVE_APPN_ICC: i32 = 1 << 2;
/// `HAVE_APPn_Exif`.
pub const HAVE_APPN_EXIF: i32 = 1 << 3;
/// `HAVE_APPn_XMP`.
pub const HAVE_APPN_XMP: i32 = 1 << 4;

/// `JPEG_EXIF_BIGENDIAN`.
pub const JPEG_EXIF_BIGENDIAN: i32 = 0;
/// `JPEG_EXIF_LITTLEENDIAN`.
pub const JPEG_EXIF_LITTLEENDIAN: i32 = 1;
pub const JPEG_EXIF_TYPE_BYTE: i32 = 1;
pub const JPEG_EXIF_TYPE_ASCII: i32 = 2;
pub const JPEG_EXIF_TYPE_SHORT: i32 = 3;
pub const JPEG_EXIF_TYPE_LONG: i32 = 4;
pub const JPEG_EXIF_TYPE_RATIONAL: i32 = 5;
pub const JPEG_EXIF_TYPE_UNDEFINED: i32 = 7;
pub const JPEG_EXIF_TYPE_SLONG: i32 = 9;
pub const JPEG_EXIF_TYPE_SRATIONAL: i32 = 10;
pub const JPEG_EXIF_TAG_XRESOLUTION: i32 = 282;
pub const JPEG_EXIF_TAG_YRESOLUTION: i32 = 283;
pub const JPEG_EXIF_TAG_RESOLUTIONUNIT: i32 = 296;
pub const JPEG_EXIF_TAG_RESUNIT_MS: i32 = 0x5110;
pub const JPEG_EXIF_TAG_XRES_MS: i32 = 0x5111;
pub const JPEG_EXIF_TAG_YRES_MS: i32 = 0x5112;

/// `JPEG_APPn_sig`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JpegAppnSig {
    Jfif,
    Adobe,
    Icc,
    Xmp,
}

/// `struct JPEG_APPn_JFIF` (APP0).
#[derive(Clone, Debug, Default)]
pub struct JpegAppnJfif {
    pub version: u16,
    /// 0: aspect ratio only, 1: dots per inch, 2: dots per cm.
    pub units: u8,
    pub xdensity: u16,
    pub ydensity: u16,
    pub xthumbnail: u8,
    pub ythumbnail: u8,
    pub thumbnail: Vec<u8>,
}

/// `struct JPEG_APPn_ICC` (APP2).
#[derive(Clone, Debug, Default)]
pub struct JpegAppnIcc {
    pub seq_id: u8,
    pub num_chunks: u8,
    /// The ICC profile data in this chunk (`length` is its length).
    pub chunk: Vec<u8>,
}

/// `struct JPEG_APPn_Adobe` (APP14).
#[derive(Clone, Debug, Default)]
pub struct JpegAppnAdobe {
    pub version: u16,
    pub flag0: u16,
    pub flag1: u16,
    /// Color transform code.
    pub transform: u8,
}

/// `struct JPEG_APPn_XMP` (APP1).
#[derive(Clone, Debug, Default)]
pub struct JpegAppnXmp {
    /// The XMP packet (`length` is its length).
    pub packet: Vec<u8>,
}

/// `JPEG_ext`'s `void *app_data`, by `app_sig`.
#[derive(Clone, Debug)]
pub enum JpegAppnData {
    Jfif(JpegAppnJfif),
    Adobe(JpegAppnAdobe),
    Icc(JpegAppnIcc),
    Xmp(JpegAppnXmp),
}

/// `struct JPEG_ext`.
#[derive(Clone, Debug)]
pub struct JpegExt {
    pub marker: JpegMarker,
    pub app_sig: JpegAppnSig,
    pub app_data: JpegAppnData,
}

/// `struct JPEG_info`. `Default` leaves `skipbits` empty (it
/// must be `MAX_COUNT / 8 + 1` zeros: use [`JpegInfo::jpeg_info_init`]).
#[derive(Clone, Debug, Default)]
pub struct JpegInfo {
    pub height: u16,
    pub width: u16,
    pub bits_per_component: u8,
    pub num_components: u8,
    pub xdpi: f64,
    pub ydpi: f64,
    /// `HAVE_APPn_*`.
    pub flags: i32,
    /// `appn` (`num_appn` is the length).
    pub appn: Vec<JpegExt>,
    /// `char skipbits[MAX_COUNT / 8 + 1]`: chunks not to copy.
    pub skipbits: Vec<u8>,
}

impl JpegInfo {
    /// `JPEG_info_init`.
    #[must_use]
    pub fn jpeg_info_init() -> Self {
        JpegInfo {
            height: 0,
            width: 0,
            bits_per_component: 0,
            num_components: 0,
            xdpi: 0.0,
            ydpi: 0.0,
            flags: 0,
            appn: Vec::new(),
            skipbits: vec![0; MAX_COUNT / 8 + 1],
        }
    }
    /// `JPEG_info_clear` (`JPEG_release_APPn_data` is `Drop`).
    pub fn jpeg_info_clear(&mut self) {
        self.appn = Vec::new();
        self.flags = 0;
    }
}

/// `fread(buf, 1, n, fp)` into a buffer C allocated with `NEW`: the
/// bytes past a short read are not defined in C (0 here).
fn fread_n(fp: &mut MemFile, n: usize) -> Vec<u8> {
    let mut v = fp.read(n).to_vec();
    v.resize(n, 0);
    v
}

/// `seek_relative` (`fseek(fp, d, SEEK_CUR)`): a seek before the start
/// fails and leaves the position.
fn seek_rel(fp: &mut MemFile, d: i64) {
    let to = fp.tell() as i64 + d;
    if to >= 0 {
        fp.seek_absolute(to as usize);
    }
}

/// `check_for_jpeg`: 1 if the file starts with SOI.
pub fn check_for_jpeg(fp: &mut MemFile) -> i32 {
    fp.rewind();
    let sig = fp.read(2);
    if sig.len() != 2 || sig[0] != 0xff || i32::from(sig[1]) != JM_SOI {
        return 0;
    }

    1
}

/// `JPEG_get_marker` (static): the marker, or -1.
#[allow(non_snake_case)]
pub fn JPEG_get_marker(fp: &mut MemFile) -> JpegMarker {
    let c = fp.getc();
    if c != 255 {
        return -1;
    }

    loop {
        let c = fp.getc();
        if c < 0 {
            return -1;
        } else if c > 0 && c < 255 {
            return c;
        }
    }
}

/// `add_APPn_marker` (static): the new count.
#[allow(non_snake_case)]
pub fn add_APPn_marker(
    j_info: &mut JpegInfo,
    marker: JpegMarker,
    app_sig: JpegAppnSig,
    app_data: JpegAppnData,
) -> i32 {
    let n = j_info.appn.len() as i32;

    j_info.appn.push(JpegExt {
        marker,
        app_sig,
        app_data,
    });

    n
}

/// `read_APP14_Adobe` (static): bytes read.
#[allow(non_snake_case)]
pub fn read_APP14_Adobe(j_info: &mut JpegInfo, fp: &mut MemFile) -> Result<u16> {
    let app_data = JpegAppnAdobe {
        version: fp.get_unsigned_pair()?,
        flag0: fp.get_unsigned_pair()?,
        flag1: fp.get_unsigned_pair()?,
        transform: fp.get_unsigned_byte()?,
    };

    add_APPn_marker(
        j_info,
        JM_APP14,
        JpegAppnSig::Adobe,
        JpegAppnData::Adobe(app_data),
    );

    Ok(7)
}

/// `read_exif_bytes` (static): an `n`-byte number at `buf[*p..]`,
/// advancing `*p`.
pub fn read_exif_bytes(buf: &[u8], p: &mut usize, n: i32, endian: i32) -> i32 {
    let mut rval: i32 = 0;
    let q = &buf[*p..];

    match endian {
        JPEG_EXIF_BIGENDIAN => {
            for i in 0..n as usize {
                rval = (rval << 8).wrapping_add(i32::from(q[i]));
            }
        }
        JPEG_EXIF_LITTLEENDIAN => {
            for i in (0..n as usize).rev() {
                rval = (rval << 8).wrapping_add(i32::from(q[i]));
            }
        }
        _ => {}
    }

    *p += n as usize;
    rval
}

/// `read_APP1_Exif` (static): reads the resolution tags; bytes read.
///
/// An offset in the data that points before the buffer (C reads there
/// whatever is in memory) is taken as an error.
#[allow(non_snake_case)]
pub fn read_APP1_Exif(j_info: &mut JpegInfo, fp: &mut MemFile, length: usize) -> usize {
    /* this doesn't save the data, just reads the tags we need */
    /* based on info from http://www.exif.org/Exif2-2.PDF */
    let buffer = fread_n(fp, length);
    let _ = exif_resolution(j_info, &buffer);
    length
}

/// The body of `read_APP1_Exif`; `None` at its `goto err`.
fn exif_resolution(j_info: &mut JpegInfo, buffer: &[u8]) -> Option<()> {
    let length = buffer.len();
    let mut value: i32;
    let mut xres = 0.0;
    let mut yres = 0.0;
    let mut res_unit = 1.0;
    let mut xres_ms: u32 = 0;
    let mut yres_ms: u32 = 0;
    let mut res_unit_ms = 0.0;

    if length < 10 {
        return None;
    }

    let endptr = length;
    let mut p = 0;
    while p < endptr && buffer[p] == 0 {
        p += 1;
    }

    if p + 8 >= endptr {
        return None;
    }
    /* TIFF header */
    let tiff_header = p;
    let endian = if buffer[p] == b'M' && buffer[p + 1] == b'M' {
        JPEG_EXIF_BIGENDIAN
    } else if buffer[p] == b'I' && buffer[p + 1] == b'I' {
        JPEG_EXIF_LITTLEENDIAN
    } else {
        warn!("{JPEG_DEBUG_STR}: Invalid value in Exif TIFF header.");
        return None;
    };
    p += 2;
    value = read_exif_bytes(buffer, &mut p, 2, endian);
    if value != 42 {
        warn!("{JPEG_DEBUG_STR}: Invalid value in Exif TIFF header.");
        return None;
    }
    /* Offset to 0th IFD */
    let offset = read_exif_bytes(buffer, &mut p, 4, endian);

    let at = tiff_header as i64 + i64::from(offset);
    if at < 0 || at + 2 >= endptr as i64 {
        return None;
    }
    p = at as usize;
    let mut num_fields = read_exif_bytes(buffer, &mut p, 2, endian);
    loop {
        let more = num_fields > 0;
        num_fields -= 1;
        if !(more && p < endptr) {
            break;
        }

        if p + 12 > endptr {
            warn!("{JPEG_DEBUG_STR}: Truncated Exif data...");
            return None;
        }
        let tag = read_exif_bytes(buffer, &mut p, 2, endian);
        let ty = read_exif_bytes(buffer, &mut p, 2, endian);
        let count = read_exif_bytes(buffer, &mut p, 4, endian);
        /* Exif data is redundant... */
        match tag {
            JPEG_EXIF_TAG_XRESOLUTION | JPEG_EXIF_TAG_YRESOLUTION => {
                if ty != JPEG_EXIF_TYPE_RATIONAL || count != 1 {
                    warn!("{JPEG_DEBUG_STR}: Invalid data for XResolution in Exif chunk.");
                    return None;
                }
                let offset = read_exif_bytes(buffer, &mut p, 4, endian);
                let vp = tiff_header as i64 + i64::from(offset);
                if vp < 0 || vp + 8 > length as i64 {
                    warn!("{JPEG_DEBUG_STR}: Invalid offset value in Exif data.");
                    return None;
                }
                let mut vp = vp as usize;
                let num = read_exif_bytes(buffer, &mut vp, 4, endian) as u32;
                let den = read_exif_bytes(buffer, &mut vp, 4, endian) as u32;
                if den > 0 {
                    if tag == JPEG_EXIF_TAG_XRESOLUTION {
                        xres = f64::from(num) / f64::from(den);
                    } else {
                        yres = f64::from(num) / f64::from(den);
                    }
                }
            }
            JPEG_EXIF_TAG_RESOLUTIONUNIT => {
                if ty != JPEG_EXIF_TYPE_SHORT || count != 1 {
                    warn!("{JPEG_DEBUG_STR}: Invalid data for ResolutionUnit in Exif chunk.");
                    return None;
                }
                value = read_exif_bytes(buffer, &mut p, 2, endian);
                p += 2;
                if value == 2 {
                    res_unit = 1.0; /* inch */
                } else if value == 3 {
                    res_unit = 2.54; /* cm */
                }
            }
            JPEG_EXIF_TAG_RESUNIT_MS => {
                /* PixelUnit */
                if ty != JPEG_EXIF_TYPE_BYTE || count != 1 {
                    warn!("{JPEG_DEBUG_STR}: Invalid data for ResolutionUnit in Exif chunk.");
                    return None;
                }
                value = read_exif_bytes(buffer, &mut p, 1, endian);
                p += 3;
                if value == 1 {
                    res_unit_ms = 0.0254; /* Unit is meter */
                } else {
                    res_unit_ms = 0.0;
                }
            }
            JPEG_EXIF_TAG_XRES_MS => {
                /* PixelPerUnitX */
                if ty != JPEG_EXIF_TYPE_LONG || count != 1 {
                    warn!("{JPEG_DEBUG_STR}: Invalid data for PixelPerUnitX in Exif chunk.");
                    return None;
                }
                value = read_exif_bytes(buffer, &mut p, 4, endian);
                xres_ms = value as u32;
            }
            JPEG_EXIF_TAG_YRES_MS => {
                /* PixelPerUnitY */
                if ty != JPEG_EXIF_TYPE_LONG || count != 1 {
                    warn!("{JPEG_DEBUG_STR}: Invalid data for PixelPerUnitY in Exif chunk.");
                    return None;
                }
                value = read_exif_bytes(buffer, &mut p, 4, endian);
                yres_ms = value as u32;
            }
            _ => {
                /* 40901 ColorSpace and 42240 Gamma unsupported... */
                p += 4;
            }
        }
    }
    if num_fields > 0 {
        warn!("{JPEG_DEBUG_STR}: Truncated Exif data...");
        return None;
    }

    /* Calculate Exif resolution, if given.
     */
    let (exifxdpi, exifydpi) = if xres > 0.0 && yres > 0.0 {
        (xres * res_unit, yres * res_unit)
    } else if xres_ms > 0 && yres_ms > 0 && res_unit_ms > 0.0 {
        (
            f64::from(xres_ms) * res_unit_ms,
            f64::from(yres_ms) * res_unit_ms,
        )
    } else {
        (72.0 * res_unit, 72.0 * res_unit)
    };

    /* Do not overwrite j_info->xdpi and j_info->ydpi if they are
     * already determined in JFIF.
     */
    if j_info.xdpi < 0.1 && j_info.ydpi < 0.1 {
        j_info.xdpi = exifxdpi;
        j_info.ydpi = exifydpi;
    } else {
        let xxx1 = libm::floor(exifxdpi + 0.5);
        let xxx2 = libm::floor(j_info.xdpi + 0.5);
        let yyy1 = libm::floor(exifydpi + 0.5);
        let yyy2 = libm::floor(j_info.ydpi + 0.5);

        if xxx1 != xxx2 || yyy1 != yyy2 {
            warn!(
                "{JPEG_DEBUG_STR}: Inconsistent resolution may have specified in Exif and JFIF: {xxx1}x{yyy1} - {xxx2}x{yyy2}"
            );
        }
    }

    Some(())
}

/// `read_APP0_JFIF` (static): bytes read.
#[allow(non_snake_case)]
pub fn read_APP0_JFIF(j_info: &mut JpegInfo, fp: &mut MemFile) -> Result<usize> {
    let version = fp.get_unsigned_pair()?;
    let units = fp.get_unsigned_byte()?;
    let xdensity = fp.get_unsigned_pair()?;
    let ydensity = fp.get_unsigned_pair()?;
    let xthumbnail = fp.get_unsigned_byte()?;
    let ythumbnail = fp.get_unsigned_byte()?;
    let thumb_data_len = 3 * usize::from(xthumbnail) * usize::from(ythumbnail);
    let thumbnail = if thumb_data_len > 0 {
        fread_n(fp, thumb_data_len)
    } else {
        Vec::new()
    };
    let app_data = JpegAppnJfif {
        version,
        units,
        xdensity,
        ydensity,
        xthumbnail,
        ythumbnail,
        thumbnail,
    };

    add_APPn_marker(
        j_info,
        JM_APP0,
        JpegAppnSig::Jfif,
        JpegAppnData::Jfif(app_data),
    );

    match units {
        1 => {
            j_info.xdpi = f64::from(xdensity);
            j_info.ydpi = f64::from(ydensity);
        }
        2 => {
            /* density is in pixels per cm */
            j_info.xdpi = f64::from(xdensity) * 2.54;
            j_info.ydpi = f64::from(ydensity) * 2.54;
        }
        _ => {
            /* FIXME: not sure what to do with this.... */
            j_info.xdpi = 72.0;
            j_info.ydpi = 72.0;
        }
    }

    Ok(9 + thumb_data_len)
}

/// `read_APP0_JFXX` (static): bytes read (skipped).
#[allow(non_snake_case)]
pub fn read_APP0_JFXX(fp: &mut MemFile, length: usize) -> Result<usize> {
    fp.get_unsigned_byte()?;
    /* Extension Code:
     *
     * 0x10: Thumbnail coded using JPEG
     * 0x11: Thumbnail stored using 1 byte/pixel
     * 0x13: Thumbnail stored using 3 bytes/pixel
     */
    seek_rel(fp, i64::from((length as i32).wrapping_sub(1))); /* Thunbnail image */

    /* Ignore */

    Ok(length)
}

/// `read_APP1_XMP` (static): bytes read.
#[allow(non_snake_case)]
pub fn read_APP1_XMP(j_info: &mut JpegInfo, fp: &mut MemFile, length: usize) -> usize {
    let app_data = JpegAppnXmp {
        packet: fread_n(fp, length),
    };

    add_APPn_marker(
        j_info,
        JM_APP1,
        JpegAppnSig::Xmp,
        JpegAppnData::Xmp(app_data),
    );

    length
}

/// `read_APP2_ICC` (static): bytes read.
#[allow(non_snake_case)]
pub fn read_APP2_ICC(j_info: &mut JpegInfo, fp: &mut MemFile, length: usize) -> Result<usize> {
    let seq_id = fp.get_unsigned_byte()?; /* Starting at 1 */
    let num_chunks = fp.get_unsigned_byte()?;
    let chunk = fread_n(fp, length - 2);
    let app_data = JpegAppnIcc {
        seq_id,
        num_chunks,
        chunk,
    };

    add_APPn_marker(
        j_info,
        JM_APP2,
        JpegAppnSig::Icc,
        JpegAppnData::Icc(app_data),
    );

    Ok(length)
}

/// `SET_SKIP`.
fn set_skip(j: &mut JpegInfo, c: i32) {
    if (c as usize) < MAX_COUNT {
        j.skipbits[c as usize / 8] |= 1 << (7 - (c % 8));
    }
}

/// `SKIP_CHUNK`.
fn skip_chunk(j: &JpegInfo, c: i32) -> bool {
    j.skipbits[c as usize / 8] & (1 << (7 - c % 8)) != 0
}

/// Whether `marker` is one of the SOFn markers.
fn is_sofn(marker: JpegMarker) -> bool {
    matches!(
        marker,
        JM_SOF0
            | JM_SOF1
            | JM_SOF2
            | JM_SOF3
            | JM_SOF5
            | JM_SOF6
            | JM_SOF7
            | JM_SOF9
            | JM_SOF10
            | JM_SOF11
            | JM_SOF13
            | JM_SOF14
            | JM_SOF15
    )
}

/// `fread(app_sig, 1, n, fp) == n`: the bytes, or none on a short read.
fn read_sig(fp: &mut MemFile, n: usize) -> Option<Vec<u8>> {
    let s = fp.read(n);
    (s.len() == n).then(|| s.to_vec())
}

/// `JPEG_scan_file` (static): 0, or -1 (not a JPEG / unsupported).
#[allow(non_snake_case)]
pub fn JPEG_scan_file(j_info: &mut JpegInfo, fp: &mut MemFile) -> Result<i32> {
    fp.rewind();
    let mut count: i32 = 0;
    let mut found_sofn = false;
    while !found_sofn {
        let marker = JPEG_get_marker(fp);
        if marker == -1 {
            break;
        }
        if marker != JM_SOI && !(JM_RST0..=JM_RST7).contains(&marker) {
            let mut length: i32 = i32::from(fp.get_unsigned_pair()?) - 2;
            match marker {
                m if is_sofn(m) => {
                    j_info.bits_per_component = fp.get_unsigned_byte()?;
                    j_info.height = fp.get_unsigned_pair()?;
                    j_info.width = fp.get_unsigned_pair()?;
                    j_info.num_components = fp.get_unsigned_byte()?;
                    found_sofn = true;
                }
                JM_APP0 => {
                    if length > 5 {
                        let Some(app_sig) = read_sig(fp, 5) else {
                            return Ok(-1);
                        };
                        length -= 5;
                        if app_sig == b"JFIF\0" {
                            /* APP0 JFIF marker preserved */
                            j_info.flags |= HAVE_APPN_JFIF;
                            length = length.wrapping_sub(read_APP0_JFIF(j_info, fp)? as i32);
                        } else if app_sig == b"JFXX\0" {
                            length =
                                length.wrapping_sub(read_APP0_JFXX(fp, length as usize)? as i32);
                            set_skip(j_info, count);
                        } else {
                            set_skip(j_info, count);
                        }
                    } else {
                        set_skip(j_info, count);
                    }
                    seek_rel(fp, i64::from(length));
                }
                JM_APP1 => {
                    if length > 5 {
                        let Some(app_sig) = read_sig(fp, 5) else {
                            return Ok(-1);
                        };
                        length -= 5;
                        if app_sig == b"Exif\0" {
                            /* APP1 Exif marker preserved */
                            j_info.flags |= HAVE_APPN_EXIF;
                            length = length
                                .wrapping_sub(read_APP1_Exif(j_info, fp, length as usize) as i32);
                        } else if app_sig == b"http:" && length > 24 {
                            let Some(app_sig) = read_sig(fp, 24) else {
                                return Ok(-1);
                            };
                            length -= 24;
                            if app_sig == b"//ns.adobe.com/xap/1.0/\0" {
                                j_info.flags |= HAVE_APPN_XMP;
                                length =
                                    length.wrapping_sub(
                                        read_APP1_XMP(j_info, fp, length as usize) as i32
                                    );
                            }
                            set_skip(j_info, count);
                        }
                    } else {
                        set_skip(j_info, count);
                    }
                    seek_rel(fp, i64::from(length));
                }
                JM_APP2 => {
                    if length >= 14 {
                        let Some(app_sig) = read_sig(fp, 12) else {
                            return Ok(-1);
                        };
                        length -= 12;
                        if app_sig == b"ICC_PROFILE\0" {
                            j_info.flags |= HAVE_APPN_ICC;
                            length = length
                                .wrapping_sub(read_APP2_ICC(j_info, fp, length as usize)? as i32);
                        }
                    }
                    seek_rel(fp, i64::from(length));
                    set_skip(j_info, count);
                }
                JM_APP14 => {
                    if length > 5 {
                        let Some(app_sig) = read_sig(fp, 5) else {
                            return Ok(-1);
                        };
                        length -= 5;
                        if app_sig == b"Adobe" {
                            /* APP14 Adobe marker preserved */
                            j_info.flags |= HAVE_APPN_ADOBE;
                            length = length.wrapping_sub(i32::from(read_APP14_Adobe(j_info, fp)?));
                        } else {
                            set_skip(j_info, count);
                        }
                    } else {
                        set_skip(j_info, count);
                    }
                    seek_rel(fp, i64::from(length));
                }
                _ => {
                    seek_rel(fp, i64::from(length));
                    if (JM_APP0..=JM_APP15).contains(&marker) {
                        set_skip(j_info, count);
                    }
                }
            }
        }
        count += 1;
    }

    if found_sofn { Ok(0) } else { Ok(-1) }
}

impl Dpx {
    /// `jpeg_include_image`: fills XObject `xobj_id`; 0 or -1.
    pub fn jpeg_include_image(&mut self, xobj_id: i32, fp: &mut MemFile) -> Result<i32> {
        if check_for_jpeg(fp) == 0 {
            warn!("{JPEG_DEBUG_STR}: Not a JPEG file?");
            fp.rewind();
            return Ok(-1);
        }
        /* File position is 2 here... */

        let mut info = pdf_ximage_init_image_info();

        let mut j_info = JpegInfo::jpeg_info_init();

        if JPEG_scan_file(&mut j_info, fp)? < 0 {
            warn!("{JPEG_DEBUG_STR}: Not a JPEG file?");
            j_info.jpeg_info_clear();
            return Ok(-1);
        }

        let colortype = match j_info.num_components {
            1 => PDF_COLORSPACE_TYPE_GRAY,
            3 => PDF_COLORSPACE_TYPE_RGB,
            4 => PDF_COLORSPACE_TYPE_CMYK,
            _ => {
                warn!(
                    "{JPEG_DEBUG_STR}: Unknown color space (num components: {})",
                    info.num_components
                );
                j_info.jpeg_info_clear();
                return Ok(-1);
            }
        };

        /* JPEG image use DCTDecode. */
        let stream = self.o.new_stream(0);
        let stream_dict = self.o.stream_dict(stream)?;
        self.o.put_name(stream_dict, b"Filter", b"DCTDecode")?;

        /* XMP Metadata */
        if self.o.check_version(1, 4) >= 0 && j_info.flags & HAVE_APPN_XMP != 0 {
            let xmp_stream = self.JPEG_get_XMP(&j_info)?.unwrap();
            let r = self.o.ref_obj(xmp_stream)?;
            self.o.put(stream_dict, b"Metadata", r)?;
            self.o.release(xmp_stream)?;
        }

        /* Check embedded ICC Profile */
        let mut colorspace = None;
        if j_info.flags & HAVE_APPN_ICC != 0 {
            let icc_stream = self.JPEG_get_iccp(&j_info)?;
            if let Some(icc_stream) = icc_stream {
                let profile = self.o.stream_data(icc_stream)?.to_vec();
                if self.iccp_check_colorspace(colortype, &profile) < 0 {
                    colorspace = None;
                } else {
                    let cspc_id = self.iccp_load_profile(None, /* noname */ &profile)?;
                    if cspc_id < 0 {
                        colorspace = None;
                    } else {
                        colorspace = Some(self.pdf_get_colorspace_reference(cspc_id)?);
                        let intent = self.iccp_get_rendering_intent(&profile);
                        if let Some(intent) = intent {
                            self.o.put(stream_dict, b"Intent", intent)?;
                        }
                    }
                }
                self.o.release(icc_stream)?;
            }
        }
        /* No ICC or invalid ICC profile. */
        let colorspace = match colorspace {
            Some(c) => c,
            None => match colortype {
                PDF_COLORSPACE_TYPE_GRAY => self.o.new_name(b"DeviceGray"),
                PDF_COLORSPACE_TYPE_RGB => self.o.new_name(b"DeviceRGB"),
                _ => self.o.new_name(b"DeviceCMYK"),
            },
        };
        self.o.put(stream_dict, b"ColorSpace", colorspace)?;

        /* IS_ADOBE_CMYK */
        if j_info.flags & HAVE_APPN_ADOBE != 0 && j_info.num_components == 4 {
            warn!("Adobe CMYK JPEG: Inverted color assumed.");
            let decode = self.o.new_array();
            for _ in 0..j_info.num_components {
                let one = self.o.new_number(1.0);
                self.o.add_array(decode, one)?;
                let zero = self.o.new_number(0.0);
                self.o.add_array(decode, zero)?;
            }
            self.o.put(stream_dict, b"Decode", decode)?;
        }

        /* Copy file */
        self.JPEG_copy_stream(&j_info, stream, fp)?;

        info.width = i32::from(j_info.width);
        info.height = i32::from(j_info.height);
        info.bits_per_component = i32::from(j_info.bits_per_component);
        info.num_components = i32::from(j_info.num_components);

        (info.xdensity, info.ydensity) = self.jpeg_get_density(&mut j_info);

        self.pdf_ximage_set_image(xobj_id, &info, stream)?;
        j_info.jpeg_info_clear();

        Ok(0)
    }
    /// `jpeg_get_bbox`: status, width, height, xdensity, ydensity.
    pub fn jpeg_get_bbox(&mut self, fp: &mut MemFile) -> Result<(i32, i32, i32, f64, f64)> {
        let mut j_info = JpegInfo::jpeg_info_init();

        if JPEG_scan_file(&mut j_info, fp)? < 0 {
            warn!("{JPEG_DEBUG_STR}: Not a JPEG file?");
            j_info.jpeg_info_clear();
            return Ok((-1, 0, 0, 0.0, 0.0));
        }

        let width = i32::from(j_info.width);
        let height = i32::from(j_info.height);

        let (xd, yd) = self.jpeg_get_density(&mut j_info);

        j_info.jpeg_info_clear();

        Ok((0, width, height, xd, yd))
    }
    /// `jpeg_get_density` (static): xdensity, ydensity (reads
    /// `self.conf.compat_mode`; may set `j_info`'s dpi to 72).
    pub fn jpeg_get_density(&mut self, j_info: &mut JpegInfo) -> (f64, f64) {
        if self.conf.compat_mode == CompatMode::Compat {
            return (72.0 / 100.0, 72.0 / 100.0);
        }

        /*
         * j_info->xdpi and j_info->ydpi are determined in most cases
         * in JPEG_scan_file(). FIXME: However, in some kinds of JPEG files,
         * j_info->xdpi, and j_info->ydpi are not determined in
         * JPEG_scan_file(). In this case we assume
         * that j_info->xdpi = j_info->ydpi = 72.0.
         */
        if j_info.xdpi < 0.1 && j_info.ydpi < 0.1 {
            j_info.xdpi = 72.0;
            j_info.ydpi = 72.0;
        }
        (72.0 / j_info.xdpi, 72.0 / j_info.ydpi)
    }
    /// `JPEG_get_iccp` (static): the ICC profile stream, or none.
    #[allow(non_snake_case)]
    pub fn JPEG_get_iccp(&mut self, j_info: &JpegInfo) -> Result<Option<Obj>> {
        let mut prev_id: i32 = 0;
        let mut num_icc_seg: i32 = -1;

        let icc_stream = self.o.new_stream(STREAM_COMPRESS);
        for ext in &j_info.appn {
            if ext.marker != JM_APP2 || ext.app_sig != JpegAppnSig::Icc {
                continue;
            }
            let JpegAppnData::Icc(icc) = &ext.app_data else {
                continue;
            };
            if num_icc_seg < 0 && prev_id == 0 {
                num_icc_seg = i32::from(icc.num_chunks);
                /* ICC chunks are sorted? */
            } else if i32::from(icc.seq_id) != prev_id + 1
                || num_icc_seg != i32::from(icc.num_chunks)
                || icc.seq_id > icc.num_chunks
            {
                warn!(
                    "Invalid JPEG ICC chunk: {} (p:{prev_id}, n:{})",
                    icc.seq_id, icc.num_chunks
                );
                self.o.release(icc_stream)?;
                return Ok(None);
            }
            self.o.add_stream(icc_stream, &icc.chunk)?;
            prev_id = i32::from(icc.seq_id);
            num_icc_seg = i32::from(icc.num_chunks);
        }

        Ok(Some(icc_stream))
    }
    /// `JPEG_get_XMP` (static): the XMP metadata stream, or none.
    #[allow(non_snake_case)]
    pub fn JPEG_get_XMP(&mut self, j_info: &JpegInfo) -> Result<Option<Obj>> {
        let mut count = 0;

        /* I don't know if XMP Metadata should be compressed here.*/
        let xmp_stream = self.o.new_stream(STREAM_COMPRESS);
        let stream_dict = self.o.stream_dict(xmp_stream)?;
        self.o.put_name(stream_dict, b"Type", b"Metadata")?;
        self.o.put_name(stream_dict, b"Subtype", b"XML")?;
        for ext in &j_info.appn {
            /* Not sure for the case of multiple segments */
            if ext.marker != JM_APP1 || ext.app_sig != JpegAppnSig::Xmp {
                continue;
            }
            if let JpegAppnData::Xmp(xmp) = &ext.app_data {
                self.o.add_stream(xmp_stream, &xmp.packet)?;
            }
            count += 1;
        }
        if count > 1 {
            warn!("{JPEG_DEBUG_STR}: Multiple XMP segments found in JPEG file. (untested)");
        }

        Ok(Some(xmp_stream))
    }
    /// `JPEG_copy_stream` (static): copies the chunks not skipped into
    /// `stream`; 0 or -1. (C's `COPY_CHUNK` loops for ever on a file
    /// that ends within a chunk; here the copy stops.)
    #[allow(non_snake_case)]
    pub fn JPEG_copy_stream(
        &mut self,
        j_info: &JpegInfo,
        stream: Obj,
        fp: &mut MemFile,
    ) -> Result<i32> {
        fp.rewind();
        let mut count: i32 = 0;
        let mut found_sofn = false;
        while !found_sofn && count < MAX_COUNT as i32 {
            let marker = JPEG_get_marker(fp);
            if marker == -1 {
                break;
            }
            if marker == JM_SOI || (JM_RST0..=JM_RST7).contains(&marker) {
                self.o.add_stream(stream, &[0xff, marker as u8])?;
            } else {
                let length: i32 = i32::from(fp.get_unsigned_pair()?) - 2;
                let header = [
                    0xff,
                    marker as u8,
                    (((length + 2) >> 8) & 0xff) as u8,
                    ((length + 2) & 0xff) as u8,
                ];
                if is_sofn(marker) {
                    self.o.add_stream(stream, &header)?;
                    self.jpeg_copy_chunk(fp, stream, length)?;
                    found_sofn = true;
                } else if skip_chunk(j_info, count) {
                    seek_rel(fp, i64::from(length));
                } else {
                    self.o.add_stream(stream, &header)?;
                    self.jpeg_copy_chunk(fp, stream, length)?;
                }
            }
            count += 1;
        }
        let rest = fp.read(fp.len()).to_vec();
        self.o.add_stream(stream, &rest)?;

        if found_sofn { Ok(0) } else { Ok(-1) }
    }
    /// `COPY_CHUNK`.
    fn jpeg_copy_chunk(&mut self, fp: &mut MemFile, stream: Obj, length: i32) -> Result<()> {
        if length > 0 {
            let chunk = fp.read(length as usize).to_vec();
            self.o.add_stream(stream, &chunk)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;

    /// A JPEG's markers (no image data): SOI, APP0 JFIF at 300 dpi, an
    /// APP2 that is not ICC, SOF0 of 16x8 RGB, SOS.
    fn jpeg() -> Vec<u8> {
        let mut d = vec![0xff, 0xd8];
        d.extend_from_slice(&[0xff, 0xe0, 0, 16]);
        d.extend_from_slice(b"JFIF\0\x01\x02\x01\x01\x2c\x01\x2c\0\0");
        d.extend_from_slice(&[0xff, 0xe2, 0, 6, b'a', b'b', b'c', b'd']);
        d.extend_from_slice(&[0xff, 0xc0, 0, 17, 8, 0, 8, 0, 16, 3]);
        d.extend_from_slice(&[1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
        d.extend_from_slice(&[0xff, 0xda, 0, 2, 0x55]);
        d
    }

    #[test]
    fn scan() {
        let mut fp = MemFile::new(Arc::from(jpeg()), b"t.jpg");
        assert_eq!(check_for_jpeg(&mut fp), 1);
        let mut j = JpegInfo::jpeg_info_init();
        assert_eq!(JPEG_scan_file(&mut j, &mut fp).unwrap(), 0);
        assert_eq!((j.width, j.height, j.num_components), (16, 8, 3));
        assert_eq!((j.xdpi, j.ydpi), (300.0, 300.0));
        assert_eq!(j.flags, HAVE_APPN_JFIF);
        // the APP2 (chunk 2) is skipped, the others kept
        assert_eq!(j.skipbits[0], 0b0010_0000);
    }

    #[test]
    fn exif_numbers() {
        let b = [0x12, 0x34, 0x56, 0x78];
        let mut p = 0;
        assert_eq!(read_exif_bytes(&b, &mut p, 2, JPEG_EXIF_BIGENDIAN), 0x1234);
        assert_eq!(
            read_exif_bytes(&b, &mut p, 2, JPEG_EXIF_LITTLEENDIAN),
            0x7856
        );
        assert_eq!(p, 4);
    }
}
