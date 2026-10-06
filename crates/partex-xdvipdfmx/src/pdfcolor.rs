//! pdfcolor.c, pdfcolor.h: colors, the color stack, ICC colorspaces.
//!
//! Color operations on plain `PdfColor` data are its methods. The color
//! stack and the colorspace cache (`cspc_id` indexes `cspc_cache`) are in
//! `self.color`. `pdf_color_get_current` returns copies of the current
//! stroke and fill colors (C returns pointers into the stack, only read).
//! No named-color table here: those are spc_color.c / spc_util.c's.
//! (`pdf_colorspace_load_ICCBased`, `iccp_load_file_stream`,
//! `pdf_get_colorspace_num_components`/`_subtype` and
//! `pdf_dev_preserve_color` are `#if 0` in C: not ported.)

use crate::fmt::Buf;
use crate::prelude::*;

pub const PDF_COLORSPACE_TYPE_CMYK: i32 = -4;
pub const PDF_COLORSPACE_TYPE_RGB: i32 = -3;
pub const PDF_COLORSPACE_TYPE_SPOT: i32 = -2;
pub const PDF_COLORSPACE_TYPE_GRAY: i32 = -1;
pub const PDF_COLORSPACE_TYPE_INVALID: i32 = 0;
pub const PDF_COLORSPACE_TYPE_DEVICEGRAY: i32 = 1;
pub const PDF_COLORSPACE_TYPE_DEVICERGB: i32 = 2;
pub const PDF_COLORSPACE_TYPE_DEVICECMYK: i32 = 3;
pub const PDF_COLORSPACE_TYPE_CALGRAY: i32 = 4;
pub const PDF_COLORSPACE_TYPE_CALRGB: i32 = 5;
pub const PDF_COLORSPACE_TYPE_LAB: i32 = 6;
pub const PDF_COLORSPACE_TYPE_ICCBASED: i32 = 7;
pub const PDF_COLORSPACE_TYPE_SEPARATION: i32 = 8;
pub const PDF_COLORSPACE_TYPE_DEVICEN: i32 = 9;
pub const PDF_COLORSPACE_TYPE_INDEXED: i32 = 10;
pub const PDF_COLORSPACE_TYPE_PATTERN: i32 = 11;

/// `PDF_COLOR_COMPONENT_MAX`.
pub const PDF_COLOR_COMPONENT_MAX: usize = 32;

/// `DEV_COLOR_STACK_MAX`.
pub const DEV_COLOR_STACK_MAX: usize = 128;

pub const ICC_INTENT_PERCEPTUAL: i32 = 0;
pub const ICC_INTENT_RELATIVE: i32 = 1;
pub const ICC_INTENT_SATURATION: i32 = 2;
pub const ICC_INTENT_ABSOLUTE: i32 = 3;

pub const ICC_HEAD_SECT1_START: usize = 0;
pub const ICC_HEAD_SECT1_LENGTH: usize = 56;
pub const ICC_HEAD_SECT2_START: usize = 68;
pub const ICC_HEAD_SECT2_LENGTH: usize = 16;
pub const ICC_HEAD_SECT3_START: usize = 100;
pub const ICC_HEAD_SECT3_LENGTH: usize = 28;

pub const PDF_COLORSPACE_FAMILY_DEVICE: i32 = 0;
pub const PDF_COLORSPACE_FAMILY_CIEBASED: i32 = 1;
pub const PDF_COLORSPACE_FAMILY_SPECIAL: i32 = 2;

/// `iccNullSig`.
pub const ICC_NULL_SIG: IccSig = 0;

/// `nullbytes16`.
pub const NULLBYTES16: [u8; 16] = [0; 16];

/// `icc_versions[]` (major, minor), indexed by PDF version - 10.
pub const ICC_VERSIONS: [(i32, i32); 11] = [
    (0, 0),
    (0, 0),
    (0, 0),
    (0x02, 0x10),
    (0x02, 0x20),
    (0x04, 0x00),
    (0x04, 0x00),
    (0x04, 0x20),
    (0x04, 0x20),
    (0x04, 0x20),
    (0x04, 0x20),
];

/// `pdf_color`.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfColor {
    pub res_id: i32,
    /// `PDF_COLORSPACE_TYPE_*`.
    pub r#type: i32,
    pub num_components: i32,
    pub spot_color_name: Option<Vec<u8>>,
    pub values: [f64; PDF_COLOR_COMPONENT_MAX],
    pub pattern_id: i32,
}

impl Default for PdfColor {
    /// All zero, as a C static is (`pdf_color_black` gives black gray).
    fn default() -> Self {
        PdfColor {
            res_id: 0,
            r#type: 0,
            num_components: 0,
            spot_color_name: None,
            values: [0.0; PDF_COLOR_COMPONENT_MAX],
            pattern_id: 0,
        }
    }
}

/// C's initial `current_fill`/`current_stroke`/`default_color`: gray 0,
/// `res_id` and `pattern_id` -1.
#[must_use]
pub fn pdf_color_initial() -> PdfColor {
    PdfColor {
        res_id: -1,
        r#type: PDF_COLORSPACE_TYPE_GRAY,
        num_components: 1,
        spot_color_name: None,
        values: [0.0; PDF_COLOR_COMPONENT_MAX],
        pattern_id: -1,
    }
}

/// `iccSig`.
pub type IccSig = u32;

/// `iccXYZNumber` (s15Fixed16Number).
#[derive(Clone, Copy, Debug, Default)]
pub struct IccXyzNumber {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// `iccHeader`.
#[derive(Clone, Copy, Debug, Default)]
pub struct IccHeader {
    pub size: i32,
    pub cmm_type: IccSig,
    pub version: i32,
    pub dev_class: IccSig,
    pub color_space: IccSig,
    /// Profile Connection Space.
    pub pcs: IccSig,
    pub creation_date: [u8; 12],
    pub acsp: IccSig,
    pub platform: IccSig,
    pub flags: [u8; 4],
    pub dev_mnfct: IccSig,
    pub dev_model: IccSig,
    pub dev_attr: [u8; 8],
    pub intent: i32,
    pub illuminant: IccXyzNumber,
    pub creator: IccSig,
    /// MD5 checksum.
    pub id: [u8; 16],
}

/// `struct iccbased_cdata`.
#[derive(Clone, Copy, Debug, Default)]
pub struct IccbasedCdata {
    /// 'i' 'c' 'c' 'b'.
    pub sig: i32,
    /// MD5 checksum.
    pub checksum: [u8; 16],
    /// Input colorspace (RGB, Gray, CMYK).
    pub colorspace: i32,
    /// Alternate colorspace (id), unused.
    pub alternate: i32,
}

/// `pdf_colorspace` (`cdata` is only ever an ICC-based one).
#[derive(Clone, Debug, Default)]
pub struct PdfColorspace {
    pub ident: Option<Vec<u8>>,
    pub subtype: i32,
    pub resource: Option<Obj>,
    pub reference: Option<Obj>,
    pub cdata: Option<IccbasedCdata>,
}

/// `color_stack` (`stroke`/`fill` have `DEV_COLOR_STACK_MAX` entries).
/// 20260113's stack has no `source`: push and pop take none.
#[derive(Clone, Debug)]
pub struct ColorStack {
    pub current: i32,
    pub stroke: Vec<PdfColor>,
    pub fill: Vec<PdfColor>,
}

impl Default for ColorStack {
    /// C's `{0,}`: every entry zeroed (type 0, no components).
    fn default() -> Self {
        ColorStack {
            current: 0,
            stroke: vec![PdfColor::default(); DEV_COLOR_STACK_MAX],
            fill: vec![PdfColor::default(); DEV_COLOR_STACK_MAX],
        }
    }
}

/// pdfcolor.c's globals and statics.
#[derive(Clone, Debug)]
pub struct State {
    pub current_fill: PdfColor,
    pub current_stroke: PdfColor,
    pub default_color: PdfColor,
    pub color_stack: ColorStack,
    /// `cspc_cache` (count/capacity: the Vec).
    pub cspc_cache: Vec<PdfColorspace>,
}

impl Default for State {
    fn default() -> Self {
        State {
            current_fill: pdf_color_initial(),
            current_stroke: pdf_color_initial(),
            default_color: pdf_color_initial(),
            color_stack: ColorStack::default(),
            cspc_cache: Vec::new(),
        }
    }
}

/// C's `printf("%g", v)`: 6 significant digits, trailing zeros (and a
/// trailing point) removed, the exponent form `d.ddde±XX` when the
/// exponent is below -4 or at least 6.
pub fn sprint_g(b: &mut Buf, v: f64) {
    const P: i32 = 6;
    if v.is_nan() {
        b.extend(if v.is_sign_negative() {
            b"-nan"
        } else {
            b"nan"
        });
        return;
    }
    if v.is_infinite() {
        b.extend(if v < 0.0 { b"-inf" } else { b"inf" });
        return;
    }
    if v == 0.0 {
        b.extend(if v.is_sign_negative() { b"-0" } else { b"0" });
        return;
    }
    // The exponent X of the %e conversion at precision P - 1.
    let e = format!("{:.*e}", (P - 1) as usize, v);
    let (mant, exp) = e.split_once('e').unwrap_or((e.as_str(), "0"));
    let x: i32 = exp.parse().unwrap_or(0);
    if x < -4 || x >= P {
        let mut m = mant.as_bytes().to_vec();
        strip_fraction_zeros(&mut m);
        b.extend(&m);
        b.push(b'e');
        b.push(if x < 0 { b'-' } else { b'+' });
        let ax = x.unsigned_abs();
        if ax < 10 {
            b.push(b'0');
        }
        b.uint(ax);
    } else {
        let f = format!("{:.*}", (P - 1 - x) as usize, v);
        let mut m = f.into_bytes();
        strip_fraction_zeros(&mut m);
        b.extend(&m);
    }
}

/// `%g`'s removal of trailing zeros in the fraction, and of the point.
fn strip_fraction_zeros(m: &mut Vec<u8>) {
    if m.contains(&b'.') {
        while m.last() == Some(&b'0') {
            m.pop();
        }
        if m.last() == Some(&b'.') {
            m.pop();
        }
    }
}

/// `sprintf(buf, " %g", ROUND(v, 0.001))`, pdfcolor's color values.
fn sprint_value(b: &mut Buf, v: f64) {
    b.push(b' ');
    sprint_g(b, crate::fmt::round_acc(v, 0.001));
}

impl PdfColor {
    /// `pdf_color_type`.
    #[must_use]
    pub fn pdf_color_type(&self) -> i32 {
        self.r#type
    }
    /// `pdf_color_rgbcolor`.
    pub fn pdf_color_rgbcolor(&mut self, r: f64, g: f64, b: f64) -> i32 {
        if !(0.0..=1.0).contains(&r) {
            warn!("Invalid color value specified: red={r}");
            return -1;
        }
        if !(0.0..=1.0).contains(&g) {
            warn!("Invalid color value specified: green={g}");
            return -1;
        }
        if !(0.0..=1.0).contains(&b) {
            warn!("Invalid color value specified: blue={b}");
            return -1;
        }
        self.values[0] = r;
        self.values[1] = g;
        self.values[2] = b;

        self.res_id = -1;
        self.r#type = PDF_COLORSPACE_TYPE_RGB;
        self.num_components = 3;

        self.spot_color_name = None;

        0
    }
    /// `pdf_color_cmykcolor`.
    pub fn pdf_color_cmykcolor(&mut self, c: f64, m: f64, y: f64, k: f64) -> i32 {
        if !(0.0..=1.0).contains(&c) {
            warn!("Invalid color value specified: cyan={c}");
            return -1;
        }
        if !(0.0..=1.0).contains(&m) {
            warn!("Invalid color value specified: magenta={m}");
            return -1;
        }
        if !(0.0..=1.0).contains(&y) {
            warn!("Invalid color value specified: yellow={y}");
            return -1;
        }
        if !(0.0..=1.0).contains(&k) {
            warn!("Invalid color value specified: black={k}");
            return -1;
        }

        self.values[0] = c;
        self.values[1] = m;
        self.values[2] = y;
        self.values[3] = k;

        self.res_id = -1;
        self.r#type = PDF_COLORSPACE_TYPE_CMYK;
        self.num_components = 4;

        self.spot_color_name = None;

        0
    }
    /// `pdf_color_graycolor`.
    pub fn pdf_color_graycolor(&mut self, g: f64) -> i32 {
        if !(0.0..=1.0).contains(&g) {
            warn!("Invalid color value specified: gray={g}");
            return -1;
        }

        self.values[0] = g;

        self.res_id = -1;
        self.r#type = PDF_COLORSPACE_TYPE_GRAY;
        self.num_components = 1;

        self.spot_color_name = None;

        0
    }
    /// `pdf_color_spotcolor` (C keeps the name pointer; here a copy).
    pub fn pdf_color_spotcolor(&mut self, name: &[u8], c: f64) -> i32 {
        if !(0.0..=1.0).contains(&c) {
            warn!("Invalid color value specified: grade={c}");
            return -1;
        }

        self.values[0] = c;
        self.values[1] = 0.0; /* Dummy */

        self.res_id = -1;
        self.r#type = PDF_COLORSPACE_TYPE_SPOT;
        self.num_components = 2;

        self.spot_color_name = Some(name.to_vec());

        0
    }
    /// `pdf_color_copycolor`: `self` (color1) = color2, deep copy.
    pub fn pdf_color_copycolor(&mut self, color2: &PdfColor) {
        self.clone_from(color2);
    }
    /// `pdf_color_black`.
    pub fn pdf_color_black(&mut self) -> i32 {
        self.pdf_color_graycolor(0.0)
    }
    /// `pdf_color_white`.
    pub fn pdf_color_white(&mut self) -> i32 {
        self.pdf_color_graycolor(1.0)
    }
    /// `pdf_color_brighten_color`: `self` is dst.
    pub fn pdf_color_brighten_color(&mut self, src: &PdfColor, f: f64) {
        if src.r#type != PDF_COLORSPACE_TYPE_RGB
            && src.r#type != PDF_COLORSPACE_TYPE_CMYK
            && src.r#type != PDF_COLORSPACE_TYPE_GRAY
        {
            self.pdf_color_copycolor(src);
            return;
        }

        if f == 1.0 {
            self.pdf_color_white();
        } else {
            self.pdf_color_copycolor(src);
            let mut n = src.num_components;
            let f1 = if n == 4 { 0.0 } else { f }; /* n == 4 is CMYK, others are RGB and Gray */
            let f0 = 1.0 - f;

            while n > 0 {
                n -= 1;
                self.values[n as usize] = f0 * src.values[n as usize] + f1;
            }
        }
    }
    /// `pdf_color_is_white`.
    #[must_use]
    pub fn pdf_color_is_white(&self) -> i32 {
        let f = match self.r#type {
            PDF_COLORSPACE_TYPE_GRAY | PDF_COLORSPACE_TYPE_RGB => 1.0,
            PDF_COLORSPACE_TYPE_CMYK => 0.0,
            _ => return 0,
        };

        let mut n = self.num_components;
        while n > 0 {
            n -= 1;
            if self.values[n as usize] != f {
                return 0;
            }
        }

        1
    }
    /// `pdf_color_compare`: as in C, the test of the types always holds,
    /// so this is always -1.
    #[must_use]
    #[allow(clippy::nonminimal_bool, clippy::overly_complex_bool_expr)]
    pub fn pdf_color_compare(&self, color2: &PdfColor) -> i32 {
        let (t1, t2) = (self.r#type, color2.r#type);
        if t1 != PDF_COLORSPACE_TYPE_GRAY
            || t1 != PDF_COLORSPACE_TYPE_RGB
            || t1 != PDF_COLORSPACE_TYPE_CMYK
            || t1 != PDF_COLORSPACE_TYPE_SPOT
            || t2 != PDF_COLORSPACE_TYPE_GRAY
            || t2 != PDF_COLORSPACE_TYPE_RGB
            || t2 != PDF_COLORSPACE_TYPE_CMYK
            || t2 != PDF_COLORSPACE_TYPE_SPOT
            || t1 != t2
        {
            return -1;
        }

        let mut n = self.num_components;
        while {
            n -= 1;
            n >= 0
        } {
            if self.values[n as usize] != color2.values[n as usize] {
                return -1;
            }
        }
        if let (Some(a), Some(b)) = (&self.spot_color_name, &color2.spot_color_name) {
            return crate::obj::memcmp(a, b);
        }

        0
    }
}

/// `str2iccSig`: C's `char` is signed (bytes from 0x80 sign-extend).
#[must_use]
pub fn str2icc_sig(s: &[u8]) -> IccSig {
    let c = |i: usize| i32::from(s[i] as i8);
    ((c(0) << 24) | (c(1) << 16) | (c(2) << 8) | c(3)) as IccSig
}

/// `check_sig(d, p, q, r, s)`.
#[must_use]
pub fn check_sig(d: Option<&IccbasedCdata>, p: u8, q: u8, r: u8, s: u8) -> bool {
    d.is_some_and(|d| {
        d.sig == (i32::from(p) << 24 | i32::from(q) << 16 | i32::from(r) << 8 | i32::from(s))
    })
}

/// `ICC_INTENT_TYPE(n)`.
#[must_use]
pub fn icc_intent_type(n: i32) -> i32 {
    (n >> 16) & 0xff
}

impl IccHeader {
    /// `iccp_init_iccHeader` (C's "ascp" typo kept).
    pub fn iccp_init_icc_header(&mut self) {
        self.size = 0;
        self.cmm_type = ICC_NULL_SIG;
        self.version = 0xFFFFFF;
        self.dev_class = ICC_NULL_SIG;
        self.color_space = ICC_NULL_SIG;
        self.pcs = ICC_NULL_SIG;
        self.creation_date = [0; 12];
        self.acsp = str2icc_sig(b"ascp");
        self.platform = ICC_NULL_SIG;
        self.flags = [0; 4];
        self.dev_mnfct = ICC_NULL_SIG;
        self.dev_model = ICC_NULL_SIG;
        self.dev_attr = [0; 8];
        self.intent = 0;
        self.illuminant.x = 0;
        self.illuminant.y = 0;
        self.illuminant.z = 0;
        self.creator = ICC_NULL_SIG;
        self.id = [0; 16];
    }
}

impl PdfColorspace {
    /// `pdf_init_colorspace_struct`.
    pub fn pdf_init_colorspace_struct(&mut self) {
        self.ident = None;
        self.subtype = PDF_COLORSPACE_TYPE_INVALID;

        self.resource = None;
        self.reference = None;
        self.cdata = None;
    }
}

impl IccbasedCdata {
    /// `init_iccbased_cdata`.
    pub fn init_iccbased_cdata(&mut self) {
        self.sig =
            i32::from(b'i') << 24 | i32::from(b'c') << 16 | i32::from(b'c') << 8 | i32::from(b'b');
        self.checksum = [0; 16];
        self.colorspace = PDF_COLORSPACE_TYPE_INVALID;
        self.alternate = -1;
    }
}

/// `get_num_components_iccbased`.
#[must_use]
pub fn get_num_components_iccbased(cdata: &IccbasedCdata) -> i32 {
    assert!(check_sig(Some(cdata), b'i', b'c', b'c', b'b'));

    match cdata.colorspace {
        PDF_COLORSPACE_TYPE_RGB => 3,
        PDF_COLORSPACE_TYPE_CMYK => 4,
        PDF_COLORSPACE_TYPE_GRAY => 1,
        PDF_COLORSPACE_TYPE_LAB => 3,
        _ => 0,
    }
}

/// C's `strcmp`, as far as its sign goes.
fn strcmp(a: &[u8], b: &[u8]) -> i32 {
    match a.cmp(b) {
        core::cmp::Ordering::Less => -1,
        core::cmp::Ordering::Equal => 0,
        core::cmp::Ordering::Greater => 1,
    }
}

/// `compare_iccbased`.
#[must_use]
pub fn compare_iccbased(
    ident1: Option<&[u8]>,
    cdata1: Option<&IccbasedCdata>,
    ident2: Option<&[u8]>,
    cdata2: Option<&IccbasedCdata>,
) -> i32 {
    if let (Some(c1), Some(c2)) = (cdata1, cdata2) {
        assert!(check_sig(Some(c1), b'i', b'c', b'c', b'b'));
        assert!(check_sig(Some(c2), b'i', b'c', b'c', b'b'));

        if c1.checksum != NULLBYTES16 && c2.checksum != NULLBYTES16 {
            return crate::obj::memcmp(&c1.checksum, &c2.checksum);
        }
        if c1.colorspace != c2.colorspace {
            return c1.colorspace - c2.colorspace;
        }

        /* Continue if checksum unknown and colorspace is same. */
    }

    if let (Some(i1), Some(i2)) = (ident1, ident2) {
        return strcmp(i1, i2);
    }

    /* No way to compare */
    -1
}

/// `iccp_get_checksum`: MD5 of the profile with the rendering intent,
/// header attributes and profile ID zeroed.
#[must_use]
pub fn iccp_get_checksum(profile: &[u8]) -> [u8; 16] {
    let p = profile;
    let mut d = Vec::with_capacity(p.len());
    d.extend_from_slice(&p[ICC_HEAD_SECT1_START..ICC_HEAD_SECT1_START + ICC_HEAD_SECT1_LENGTH]);
    d.extend_from_slice(&NULLBYTES16[..12]);
    d.extend_from_slice(&p[ICC_HEAD_SECT2_START..ICC_HEAD_SECT2_START + ICC_HEAD_SECT2_LENGTH]);
    d.extend_from_slice(&NULLBYTES16[..16]);
    d.extend_from_slice(&p[ICC_HEAD_SECT3_START..ICC_HEAD_SECT3_START + ICC_HEAD_SECT3_LENGTH]);

    /* body */
    d.extend_from_slice(&p[128..]);

    partex_engine::md5::md5(&d)
}

/// `sget_signed_long`.
fn sget_signed_long(p: &[u8]) -> i32 {
    (u32::from(p[0]) << 24 | u32::from(p[1]) << 16 | u32::from(p[2]) << 8 | u32::from(p[3])) as i32
}

impl Dpx {
    /// `iccp_devClass_allowed` (the color mode is read, as in C, but
    /// only its default case is compiled).
    fn iccp_dev_class_allowed(&self, dev_class: IccSig) -> i32 {
        let _colormode = self.pdf_dev_get_param(crate::pdfdev::PDF_DEV_PARAM_COLORMODE);

        if dev_class != str2icc_sig(b"scnr")
            && dev_class != str2icc_sig(b"mntr")
            && dev_class != str2icc_sig(b"prtr")
            && dev_class != str2icc_sig(b"spac")
        {
            return 0;
        }

        1
    }
    /// `iccp_version_supported` (reads the output PDF version).
    fn iccp_version_supported(&self, major: i32, minor: i32) -> i32 {
        let idx = self.o.get_version() - 10;
        if idx < 11 {
            let (vmajor, vminor) = ICC_VERSIONS[idx as usize];
            if vmajor < major {
                return 0;
            } else if vmajor == major && vminor < minor {
                return 0;
            }
            return 1;
        }

        0
    }
    /// `iccp_unpack_header`: status (0 ok, -1 error).
    fn iccp_unpack_header(&mut self, icch: &mut IccHeader, profile: &[u8], check_size: i32) -> i32 {
        let proflen = profile.len() as i32;
        if check_size != 0 && (proflen < 128 || proflen % 4 != 0) {
            warn!("Profile size: {proflen}");
            return -1;
        }

        let p = profile;
        let endptr = 128;

        icch.size = sget_signed_long(&p[0..]);
        if check_size != 0 && icch.size != proflen {
            warn!("ICC Profile size: {}(header) != {proflen}", icch.size);
            return -1;
        }
        let mut i = 4;

        icch.cmm_type = str2icc_sig(&p[i..]);
        i += 4;
        icch.version = sget_signed_long(&p[i..]);
        i += 4;
        icch.dev_class = str2icc_sig(&p[i..]);
        i += 4;
        icch.color_space = str2icc_sig(&p[i..]);
        i += 4;
        icch.pcs = str2icc_sig(&p[i..]);
        i += 4;
        icch.creation_date.copy_from_slice(&p[i..i + 12]);
        i += 12;
        icch.acsp = str2icc_sig(&p[i..]); /* acsp */
        if icch.acsp != str2icc_sig(b"acsp") {
            warn!("Invalid ICC profile: not \"acsp\"");
            return -1;
        }
        i += 4;
        icch.platform = str2icc_sig(&p[i..]);
        i += 4;
        icch.flags.copy_from_slice(&p[i..i + 4]);
        i += 4;
        icch.dev_mnfct = str2icc_sig(&p[i..]);
        i += 4;
        icch.dev_model = str2icc_sig(&p[i..]);
        i += 4;
        icch.dev_attr.copy_from_slice(&p[i..i + 8]);
        i += 8;
        icch.intent = sget_signed_long(&p[i..]);
        i += 4;
        icch.illuminant.x = sget_signed_long(&p[i..]);
        i += 4;
        icch.illuminant.y = sget_signed_long(&p[i..]);
        i += 4;
        icch.illuminant.z = sget_signed_long(&p[i..]);
        i += 4;
        icch.creator = str2icc_sig(&p[i..]);
        i += 4;
        icch.id.copy_from_slice(&p[i..i + 16]);
        i += 16;

        /* 28 bytes reserved - must be set to zeros */
        while i < endptr {
            if p[i] != 0 {
                warn!(
                    "Reserved pad not zero: {:02x} (at offset {i} in ICC profile header.)",
                    p[i]
                );
                return -1;
            }
            i += 1;
        }

        0
    }
    /// `print_iccp_header` (verbose output only: nothing to print).
    fn print_iccp_header(&mut self, icch: &IccHeader, checksum: Option<&[u8; 16]>) {
        let _ = (icch, checksum);
    }
    /// `iccp_check_colorspace`.
    pub fn iccp_check_colorspace(&mut self, colortype: i32, profile: &[u8]) -> i32 {
        if profile.len() < 128 {
            return -1;
        }

        let colorspace = str2icc_sig(&profile[16..]);

        match colortype {
            PDF_COLORSPACE_TYPE_CALRGB | PDF_COLORSPACE_TYPE_RGB => {
                if colorspace != str2icc_sig(b"RGB ") {
                    return -1;
                }
            }
            PDF_COLORSPACE_TYPE_CALGRAY | PDF_COLORSPACE_TYPE_GRAY => {
                if colorspace != str2icc_sig(b"GRAY") {
                    return -1;
                }
            }
            PDF_COLORSPACE_TYPE_CMYK => {
                if colorspace != str2icc_sig(b"CMYK") {
                    return -1;
                }
            }
            _ => return -1,
        }

        0
    }
    /// `iccp_get_rendering_intent`.
    pub fn iccp_get_rendering_intent(&mut self, profile: &[u8]) -> Option<Obj> {
        if profile.len() < 128 {
            return None;
        }

        let intent = sget_signed_long(&profile[64..]);
        match icc_intent_type(intent) {
            ICC_INTENT_SATURATION => Some(self.o.new_name(b"Saturation")),
            ICC_INTENT_PERCEPTUAL => Some(self.o.new_name(b"Perceptual")),
            ICC_INTENT_ABSOLUTE => Some(self.o.new_name(b"AbsoluteColorimetric")),
            ICC_INTENT_RELATIVE => Some(self.o.new_name(b"RelativeColorimetric")),
            t => {
                warn!("Invalid rendering intent type: {t}");
                None
            }
        }
    }
    /// `iccp_load_profile`: the colorspace id, or -1.
    pub fn iccp_load_profile(&mut self, ident: Option<&[u8]>, profile: &[u8]) -> i32 {
        let mut icch = IccHeader::default();
        icch.iccp_init_icc_header();
        if self.iccp_unpack_header(&mut icch, profile, 1) < 0 {
            /* check size */
            warn!("Invalid ICC profile header");
            self.print_iccp_header(&icch, None);
            return -1;
        }

        if self.iccp_version_supported((icch.version >> 24) & 0xff, (icch.version >> 16) & 0xff)
            == 0
        {
            warn!("ICC profile format spec. version not supported in current PDF version setting.");
            warn!("ICC profile not embedded.");
            self.print_iccp_header(&icch, None);
            return -1;
        }

        if self.iccp_dev_class_allowed(icch.dev_class) == 0 {
            warn!("Unsupported ICC Profile Device Class:");
            self.print_iccp_header(&icch, None);
            return -1;
        }

        let colorspace = if icch.color_space == str2icc_sig(b"RGB ") {
            PDF_COLORSPACE_TYPE_RGB
        } else if icch.color_space == str2icc_sig(b"GRAY") {
            PDF_COLORSPACE_TYPE_GRAY
        } else if icch.color_space == str2icc_sig(b"CMYK") {
            PDF_COLORSPACE_TYPE_CMYK
        } else {
            warn!("Unsupported input color space.");
            self.print_iccp_header(&icch, None);
            return -1;
        };

        let checksum = iccp_get_checksum(profile);
        if icch.id != NULLBYTES16 && icch.id != checksum {
            warn!("Invalid ICC profile: Inconsistent checksum.");
            self.print_iccp_header(&icch, Some(&checksum));
            return -1;
        }

        let mut cdata = IccbasedCdata::default();
        cdata.init_iccbased_cdata();
        cdata.colorspace = colorspace;
        cdata.checksum = checksum;

        let cspc_id =
            self.pdf_colorspace_findresource(ident, PDF_COLORSPACE_TYPE_ICCBASED, Some(&cdata));
        if cspc_id >= 0 {
            return cspc_id;
        }
        if self.conf.verbose_level > 1 {
            self.print_iccp_header(&icch, Some(&checksum));
        }

        let resource = self.o.new_array();

        let stream = self.o.new_stream(crate::obj::STREAM_COMPRESS);
        let n = self.o.new_name(b"ICCBased");
        self.o.add_array(resource, n);
        let r = self.o.ref_obj(stream);
        self.o.add_array(resource, r);

        let stream_dict = self.o.stream_dict(stream);
        let n = f64::from(get_num_components_iccbased(&cdata));
        self.o.put_number(stream_dict, b"N", n);

        self.o.add_stream(stream, profile);
        self.o.release(stream);

        self.pdf_colorspace_defineresource(
            ident,
            PDF_COLORSPACE_TYPE_ICCBASED,
            Some(cdata),
            resource,
        )
    }
    /// `pdf_colorspace_findresource`: the id, or -1.
    fn pdf_colorspace_findresource(
        &mut self,
        ident: Option<&[u8]>,
        subtype: i32,
        cdata: Option<&IccbasedCdata>,
    ) -> i32 {
        let mut cmp = -1;
        let mut cspc_id = 0;
        while cmp != 0 && (cspc_id as usize) < self.color.cspc_cache.len() {
            let colorspace = &self.color.cspc_cache[cspc_id as usize];
            if colorspace.subtype != subtype {
                cspc_id += 1;
                continue;
            }

            if colorspace.subtype == PDF_COLORSPACE_TYPE_ICCBASED {
                cmp = compare_iccbased(
                    ident,
                    cdata,
                    colorspace.ident.as_deref(),
                    colorspace.cdata.as_ref(),
                );
            }
            if cmp == 0 {
                return cspc_id;
            }
            cspc_id += 1;
        }

        -1 /* not found */
    }
    /// `pdf_clean_colorspace_struct`: releases its objects.
    fn pdf_clean_colorspace_struct(&mut self, colorspace: &mut PdfColorspace) {
        colorspace.ident = None;
        if let Some(r) = colorspace.resource.take() {
            self.o.release(r);
        }
        if let Some(r) = colorspace.reference.take() {
            self.o.release(r);
        }

        if let Some(cdata) = &colorspace.cdata
            && colorspace.subtype == PDF_COLORSPACE_TYPE_ICCBASED
        {
            assert!(check_sig(Some(cdata), b'i', b'c', b'c', b'b'));
        }
        colorspace.cdata = None;
        colorspace.subtype = PDF_COLORSPACE_TYPE_INVALID;
    }
    /// `pdf_flush_colorspace`: releases its objects.
    fn pdf_flush_colorspace(&mut self, colorspace: &mut PdfColorspace) {
        if let Some(r) = colorspace.resource.take() {
            self.o.release(r);
        }
        if let Some(r) = colorspace.reference.take() {
            self.o.release(r);
        }
    }
    /// `pdf_colorspace_defineresource`: the new id.
    fn pdf_colorspace_defineresource(
        &mut self,
        ident: Option<&[u8]>,
        subtype: i32,
        cdata: Option<IccbasedCdata>,
        resource: Obj,
    ) -> i32 {
        let cspc_id = self.color.cspc_cache.len() as i32;
        let mut colorspace = PdfColorspace::default();

        colorspace.pdf_init_colorspace_struct();
        if let Some(ident) = ident {
            colorspace.ident = Some(ident.to_vec());
        }
        colorspace.subtype = subtype;
        colorspace.cdata = cdata;
        colorspace.resource = Some(resource);

        self.color.cspc_cache.push(colorspace);

        cspc_id
    }
    /// `pdf_get_colorspace_reference`.
    pub fn pdf_get_colorspace_reference(&mut self, cspc_id: i32) -> Obj {
        let i = cspc_id as usize;
        if self.color.cspc_cache[i].reference.is_none() {
            let resource = self.color.cspc_cache[i]
                .resource
                .expect("pdf_get_colorspace_reference: no resource");
            let r = self.o.ref_obj(resource);
            self.color.cspc_cache[i].reference = Some(r);
            self.o.release(resource); /* .... */
            self.color.cspc_cache[i].resource = None;
        }

        let r = self.color.cspc_cache[i].reference.unwrap();
        self.o.link(r)
    }
    /// `pdf_init_colors`.
    pub fn pdf_init_colors(&mut self) {
        self.color.cspc_cache = Vec::new();
    }
    /// `pdf_close_colors`.
    pub fn pdf_close_colors(&mut self) {
        let mut cache = core::mem::take(&mut self.color.cspc_cache);
        for colorspace in &mut cache {
            self.pdf_flush_colorspace(colorspace);
            self.pdf_clean_colorspace_struct(colorspace);
        }
    }
    /// `pdf_color_set_color`: appends the operators (`mask` 0 or 0x20);
    /// bytes written. A `Dpx` method: the colorspace and pattern cases
    /// add page resources. (C checks an estimate against `buffer_len`;
    /// callers pass 1024, `FORMAT_BUFF_LEN`.)
    pub fn pdf_color_set_color(
        &mut self,
        color: &PdfColor,
        buf: &mut Buf,
        buffer_len: usize,
        mask: u8,
    ) -> Result<usize> {
        let start = buf.len();
        {
            let mut estimate = 0usize;
            if color.num_components > 0 {
                estimate += 5 * (color.num_components as usize + 1) + 4; /* Assuming color values [0, 1]... */
            }
            estimate += b" /DeiceGray CS".len();
            if estimate + 1 > buffer_len {
                warn!("Not enough buffer space allocated for writing set_color op...");
                return Ok(0);
            }
        }
        let n = color.num_components.max(0) as usize;

        match color.pdf_color_type() {
            PDF_COLORSPACE_TYPE_DEVICEGRAY
            | PDF_COLORSPACE_TYPE_DEVICERGB
            | PDF_COLORSPACE_TYPE_DEVICECMYK => {
                buf.extend(match color.pdf_color_type() {
                    PDF_COLORSPACE_TYPE_DEVICEGRAY => b" /DeviceGray ",
                    PDF_COLORSPACE_TYPE_DEVICERGB => b" /DeviceRGB ",
                    _ => b" /DeviceCMYK ",
                });
                buf.push(b'C' | mask);
                buf.push(b'S' | mask);
                for i in 0..n {
                    sprint_value(buf, color.values[i]);
                }
                buf.push(b' ');
                buf.push(b'S' | mask);
                buf.push(b'C' | mask);
            }
            PDF_COLORSPACE_TYPE_GRAY => {
                for i in 0..n {
                    sprint_value(buf, color.values[i]);
                }
                buf.push(b' ');
                buf.push(b'G' | mask);
            }
            PDF_COLORSPACE_TYPE_RGB => {
                for i in 0..n {
                    sprint_value(buf, color.values[i]);
                }
                buf.push(b' ');
                buf.push(b'R' | mask);
                buf.push(b'G' | mask);
            }
            PDF_COLORSPACE_TYPE_CMYK => {
                for i in 0..n {
                    sprint_value(buf, color.values[i]);
                }
                buf.push(b' ');
                buf.push(b'K' | mask);
            }
            PDF_COLORSPACE_TYPE_SPOT => {
                buf.extend(b" /");
                buf.extend(color.spot_color_name.as_deref().unwrap_or(b"(null)"));
                buf.push(b' ');
                buf.push(b'C' | mask);
                buf.push(b'S' | mask);
                sprint_value(buf, color.values[0]);
                buf.push(b' ');
                buf.push(b'S' | mask);
                buf.push(b'C' | mask);
            }
            PDF_COLORSPACE_TYPE_CALGRAY
            | PDF_COLORSPACE_TYPE_CALRGB
            | PDF_COLORSPACE_TYPE_LAB
            | PDF_COLORSPACE_TYPE_INDEXED => {
                let res_name = res_name(b"XC", color.res_id & 0xffff, 15);
                buf.extend(b" /");
                buf.extend(&res_name);
                buf.push(b' ');
                buf.push(b'C' | mask);
                buf.push(b'S' | mask);
                for i in 0..n {
                    sprint_value(buf, color.values[i]);
                }
                buf.push(b' ');
                buf.push(b'S' | mask);
                buf.push(b'C' | mask);
                let r = self
                    .pdf_get_resource_reference(color.res_id)?
                    .expect("pdf_color_set_color: no ColorSpace resource");
                self.pdf_doc_add_page_resource(b"ColorSpace", &res_name, r)?;
            }
            PDF_COLORSPACE_TYPE_PATTERN => {
                if color.res_id < 0 {
                    buf.extend(b" /Pattern ");
                    buf.push(b'C' | mask);
                    buf.push(b'S' | mask);
                    /* no color value but just a name */
                } else {
                    let res_name = res_name(b"XC", color.res_id & 0xffff, 15);
                    buf.extend(b" /");
                    buf.extend(&res_name);
                    buf.push(b' ');
                    buf.push(b'C' | mask);
                    buf.push(b'S' | mask);
                    for i in 0..n {
                        sprint_value(buf, color.values[i]);
                    }
                    let r = self
                        .pdf_get_resource_reference(color.res_id)?
                        .expect("pdf_color_set_color: no ColorSpace resource");
                    self.pdf_doc_add_page_resource(b"ColorSpace", &res_name, r)?;
                }
                let res_name = res_name(b"XP", color.pattern_id & 0xffff, 15);
                buf.extend(b" /");
                buf.extend(&res_name);
                buf.push(b' ');
                buf.push(b'S' | mask);
                buf.push(b'C' | mask);
                buf.push(b'N' | mask);

                let r = self
                    .pdf_get_resource_reference(color.pattern_id)?
                    .expect("pdf_color_set_color: no Pattern resource");
                self.pdf_doc_add_page_resource(b"Pattern", &res_name, r)?;
            }
            _ => {
                let res_name = res_name(b"XC", color.res_id & 0xffff, 7);
                buf.extend(b" /");
                buf.extend(&res_name);
                buf.push(b' ');
                buf.push(b'C' | mask);
                buf.push(b'S' | mask);
                for i in 0..n {
                    sprint_value(buf, color.values[i]);
                }
                buf.push(b' ');
                buf.push(b'S' | mask);
                buf.push(b'C' | mask);
                buf.push(b'N' | mask);
                let r = self
                    .pdf_get_resource_reference(color.res_id)?
                    .expect("pdf_color_set_color: no ColorSpace resource");
                self.pdf_doc_add_page_resource(b"ColorSpace", &res_name, r)?;
            }
        }

        Ok(buf.len() - start)
    }
    /// `pdf_color_clear_stack`.
    pub fn pdf_color_clear_stack(&mut self) {
        let cs = &mut self.color.color_stack;
        if cs.current > 0 {
            warn!("You've mistakenly made a global color change within nested colors.");
        }
        while cs.current > 0 {
            cs.current -= 1;
            let i = cs.current as usize;
            cs.stroke[i].spot_color_name = None;
            cs.fill[i].spot_color_name = None;
        }
        cs.current = 0;
        cs.stroke[0].pdf_color_black();
        cs.fill[0].pdf_color_black();
    }
    /// `pdf_color_set`.
    pub fn pdf_color_set(&mut self, sc: &PdfColor, fc: &PdfColor) -> Result<()> {
        let cs = &mut self.color.color_stack;
        let i = cs.current as usize;
        cs.stroke[i].pdf_color_copycolor(sc);
        cs.fill[i].pdf_color_copycolor(fc);
        self.pdf_dev_reset_color(1)?;
        Ok(())
    }
    /// `pdf_color_push`.
    pub fn pdf_color_push(&mut self, sc: &PdfColor, fc: &PdfColor) -> Result<()> {
        if self.color.color_stack.current >= DEV_COLOR_STACK_MAX as i32 - 1 {
            warn!("Color stack overflow. Just ignore.");
        } else {
            self.color.color_stack.current += 1;
            self.pdf_color_set(sc, fc)?;
        }
        Ok(())
    }
    /// `pdf_color_pop`.
    pub fn pdf_color_pop(&mut self) -> Result<()> {
        if self.color.color_stack.current <= 0 {
            warn!("Color stack underflow. Just ignore.");
        } else {
            self.color.color_stack.current -= 1;
            self.pdf_dev_reset_color(1)?;
        }
        Ok(())
    }
    /// `pdf_color_get_current`: copies of (stroke, fill).
    #[must_use]
    pub fn pdf_color_get_current(&self) -> (PdfColor, PdfColor) {
        let cs = &self.color.color_stack;
        let i = cs.current as usize;
        (cs.stroke[i].clone(), cs.fill[i].clone())
    }
}

/// `snprintf(res_name, size + 1, "%s%d", prefix, id)`: at most `max`
/// bytes.
fn res_name(prefix: &[u8], id: i32, max: usize) -> Vec<u8> {
    let mut b = Buf::new();
    b.extend(prefix);
    b.int(id);
    b.0.truncate(max);
    b.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(v: f64) -> String {
        let mut b = Buf::new();
        sprint_g(&mut b, v);
        String::from_utf8(b.0).unwrap()
    }

    #[test]
    fn percent_g() {
        // printf("%g") as glibc prints them
        assert_eq!(g(0.0), "0");
        assert_eq!(g(-0.0), "-0");
        assert_eq!(g(1.0), "1");
        assert_eq!(g(0.5), "0.5");
        assert_eq!(g(0.123), "0.123");
        assert_eq!(g(100000.0), "100000");
        assert_eq!(g(1000000.0), "1e+06");
        assert_eq!(g(1234567.0), "1.23457e+06");
        assert_eq!(g(0.0001), "0.0001");
        assert_eq!(g(0.00001), "1e-05");
        assert_eq!(g(0.000123456), "0.000123456");
        assert_eq!(g(-2.5), "-2.5");
        assert_eq!(g(123456.5), "123456");
        assert_eq!(g(123457.5), "123458");
        assert_eq!(g(999999.5), "1e+06");
        assert_eq!(g(1e100), "1e+100");
        assert_eq!(g(1.5e-300), "1.5e-300");
        assert_eq!(g(0.30000000000000004), "0.3");
    }

    #[test]
    fn color_values() {
        // " %g" of ROUND(v, 0.001)
        let mut b = Buf::new();
        sprint_value(&mut b, 0.12345);
        sprint_value(&mut b, 1.0 / 3.0);
        sprint_value(&mut b, 0.9999);
        sprint_value(&mut b, 0.0004);
        sprint_value(&mut b, 0.0005);
        assert_eq!(b.0, b" 0.123 0.333 1 0 0.001");
    }

    #[test]
    fn icc_sigs() {
        assert_eq!(str2icc_sig(b"acsp"), 0x6163_7370);
        // C's signed char: a byte from 0x80 sign-extends over the others
        assert_eq!(str2icc_sig(b"ab\xffd"), 0xffff_ff64);
        assert_eq!(icc_intent_type(0x0001_0000), 1);
        let mut c = IccbasedCdata::default();
        c.init_iccbased_cdata();
        assert!(check_sig(Some(&c), b'i', b'c', b'c', b'b'));
    }

    #[test]
    fn brighten_and_white() {
        let mut src = PdfColor::default();
        src.pdf_color_rgbcolor(0.2, 0.4, 1.0);
        let mut dst = PdfColor::default();
        dst.pdf_color_brighten_color(&src, 0.5);
        assert_eq!(&dst.values[..3], &[0.6, 0.7, 1.0]);
        let mut w = PdfColor::default();
        w.pdf_color_white();
        assert_eq!(w.pdf_color_is_white(), 1);
        assert_eq!(w.pdf_color_compare(&w.clone()), -1);
    }
}
