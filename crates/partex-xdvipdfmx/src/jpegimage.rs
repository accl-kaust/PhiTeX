//! jpegimage.c, jpegimage.h: JPEG images (copied as DCTDecode streams).

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
        todo!()
    }
    /// `JPEG_info_clear` (`JPEG_release_APPn_data` is `Drop`).
    pub fn jpeg_info_clear(&mut self) {
        todo!()
    }
}

/// `check_for_jpeg`: 1 if the file starts with SOI.
pub fn check_for_jpeg(fp: &mut MemFile) -> i32 {
    todo!()
}

/// `JPEG_get_marker` (static): the marker, or -1.
#[allow(non_snake_case)]
pub fn JPEG_get_marker(fp: &mut MemFile) -> JpegMarker {
    todo!()
}

/// `add_APPn_marker` (static): the new count.
#[allow(non_snake_case)]
pub fn add_APPn_marker(
    j_info: &mut JpegInfo,
    marker: JpegMarker,
    app_sig: JpegAppnSig,
    app_data: JpegAppnData,
) -> i32 {
    todo!()
}

/// `read_APP14_Adobe` (static): bytes read.
#[allow(non_snake_case)]
pub fn read_APP14_Adobe(j_info: &mut JpegInfo, fp: &mut MemFile) -> u16 {
    todo!()
}

/// `read_exif_bytes` (static): an `n`-byte number at `buf[*p..]`,
/// advancing `*p`.
pub fn read_exif_bytes(buf: &[u8], p: &mut usize, n: i32, endian: i32) -> i32 {
    todo!()
}

/// `read_APP1_Exif` (static): reads the resolution tags; bytes read.
#[allow(non_snake_case)]
pub fn read_APP1_Exif(j_info: &mut JpegInfo, fp: &mut MemFile, length: usize) -> usize {
    todo!()
}

/// `read_APP0_JFIF` (static): bytes read.
#[allow(non_snake_case)]
pub fn read_APP0_JFIF(j_info: &mut JpegInfo, fp: &mut MemFile) -> usize {
    todo!()
}

/// `read_APP0_JFXX` (static): bytes read (skipped).
#[allow(non_snake_case)]
pub fn read_APP0_JFXX(fp: &mut MemFile, length: usize) -> usize {
    todo!()
}

/// `read_APP1_XMP` (static): bytes read.
#[allow(non_snake_case)]
pub fn read_APP1_XMP(j_info: &mut JpegInfo, fp: &mut MemFile, length: usize) -> usize {
    todo!()
}

/// `read_APP2_ICC` (static): bytes read.
#[allow(non_snake_case)]
pub fn read_APP2_ICC(j_info: &mut JpegInfo, fp: &mut MemFile, length: usize) -> usize {
    todo!()
}

/// `JPEG_scan_file` (static): 0, or -1 (not a JPEG / unsupported).
#[allow(non_snake_case)]
pub fn JPEG_scan_file(j_info: &mut JpegInfo, fp: &mut MemFile) -> i32 {
    todo!()
}

impl Dpx {
    /// `jpeg_include_image`: fills XObject `xobj_id`; 0 or -1.
    pub fn jpeg_include_image(&mut self, xobj_id: i32, fp: &mut MemFile) -> i32 {
        todo!()
    }
    /// `jpeg_get_bbox`: status, width, height, xdensity, ydensity.
    pub fn jpeg_get_bbox(&mut self, fp: &mut MemFile) -> (i32, i32, i32, f64, f64) {
        todo!()
    }
    /// `jpeg_get_density` (static): xdensity, ydensity (reads
    /// `self.conf.compat_mode`; may set `j_info`'s dpi to 72).
    pub fn jpeg_get_density(&mut self, j_info: &mut JpegInfo) -> (f64, f64) {
        todo!()
    }
    /// `JPEG_get_iccp` (static): the ICC profile stream, or none.
    #[allow(non_snake_case)]
    pub fn JPEG_get_iccp(&mut self, j_info: &JpegInfo) -> Option<Obj> {
        todo!()
    }
    /// `JPEG_get_XMP` (static): the XMP metadata stream, or none.
    #[allow(non_snake_case)]
    pub fn JPEG_get_XMP(&mut self, j_info: &JpegInfo) -> Option<Obj> {
        todo!()
    }
    /// `JPEG_copy_stream` (static): copies the chunks not skipped into
    /// `stream`; 0 or -1.
    #[allow(non_snake_case)]
    pub fn JPEG_copy_stream(&mut self, j_info: &JpegInfo, stream: Obj, fp: &mut MemFile) -> i32 {
        todo!()
    }
}
