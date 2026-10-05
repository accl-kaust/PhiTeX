//! fontmap.c, fontmap.h: the font map (`.map` files, `\special{pdf:mapline}`,
//! XeTeX's native fonts).
//!
//! The map is `self.fontmap.fontmap`, an [`HtTable`] of [`FontmapRec`]s
//! keyed by TeX font name (`None` before `pdf_init_fontmaps`). Lookups
//! return an owned copy of the record. SFD subfonts (`foo@SFD@`) need
//! subfont.c, which is not ported: those paths are `todo!()`.

use crate::dpxutil::HtTable;
use crate::prelude::*;

/// `FONTMAP_RMODE_REPLACE`.
pub const FONTMAP_RMODE_REPLACE: i32 = 0;
/// `FONTMAP_RMODE_APPEND`.
pub const FONTMAP_RMODE_APPEND: i32 = b'+' as i32;
/// `FONTMAP_RMODE_REMOVE`.
pub const FONTMAP_RMODE_REMOVE: i32 = b'-' as i32;

/// `FONTMAP_OPT_NOEMBED`.
pub const FONTMAP_OPT_NOEMBED: i32 = 1 << 1;
/// `FONTMAP_OPT_VERT`.
pub const FONTMAP_OPT_VERT: i32 = 1 << 2;

/// `FONTMAP_STYLE_NONE`.
pub const FONTMAP_STYLE_NONE: i32 = 0;
/// `FONTMAP_STYLE_BOLD`.
pub const FONTMAP_STYLE_BOLD: i32 = 1;
/// `FONTMAP_STYLE_ITALIC`.
pub const FONTMAP_STYLE_ITALIC: i32 = 2;
/// `FONTMAP_STYLE_BOLDITALIC`.
pub const FONTMAP_STYLE_BOLDITALIC: i32 = 3;

/// `CID_MAPREC_CSI_DELIM`.
pub const CID_MAPREC_CSI_DELIM: u8 = b'/';

/// `fontmap_opt`.
#[derive(Clone, Debug, PartialEq)]
pub struct FontmapOpt {
    pub slant: f64,
    pub extend: f64,
    pub bold: f64,
    pub mapc: i32,
    pub flags: i32,
    pub otl_tags: Option<Vec<u8>>,
    pub tounicode: Option<Vec<u8>>,
    pub design_size: f64,
    /// Adobe-Japan1-4, etc.
    pub charcoll: Option<Vec<u8>>,
    /// TTC index.
    pub index: u32,
    pub style: i32,
    pub stemv: i32,
    pub use_glyph_encoding: i32,
}

/// `pdf_init_fontmap_record`'s values.
impl Default for FontmapOpt {
    fn default() -> Self {
        FontmapOpt {
            slant: 0.0,
            extend: 1.0,
            bold: 0.0,
            mapc: -1,
            flags: 0,
            otl_tags: None,
            tounicode: None,
            design_size: -1.0,
            charcoll: None,
            index: 0,
            style: FONTMAP_STYLE_NONE,
            stemv: -1,
            use_glyph_encoding: 0,
        }
    }
}

/// `fontmap_rec`'s `charmap` (SFD subfont mapping).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FontmapCharmap {
    pub sfd_name: Option<Vec<u8>>,
    pub subfont_id: Option<Vec<u8>>,
}

/// `fontmap_rec`. `Default` is `pdf_init_fontmap_record`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FontmapRec {
    pub map_name: Option<Vec<u8>>,
    pub font_name: Option<Vec<u8>>,
    pub enc_name: Option<Vec<u8>>,
    pub charmap: FontmapCharmap,
    pub opt: FontmapOpt,
}

/// fontmap.c's statics.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// `fontmap` (NULL before `pdf_init_fontmaps`).
    pub fontmap: Option<HtTable<FontmapRec>>,
}

/// `pdf_init_fontmap_record`.
pub fn pdf_init_fontmap_record(mrec: &mut FontmapRec) {
    *mrec = FontmapRec::default();
}

/// `pdf_clear_fontmap_record`.
pub fn pdf_clear_fontmap_record(mrec: &mut FontmapRec) {
    pdf_init_fontmap_record(mrec);
}

/// `pdf_copy_fontmap_record` (static). C leaves `design_size` (unused)
/// as `NEW` left it; here it keeps `dst`'s.
fn pdf_copy_fontmap_record(dst: &mut FontmapRec, src: &FontmapRec) {
    dst.map_name.clone_from(&src.map_name);

    dst.charmap.sfd_name.clone_from(&src.charmap.sfd_name);
    dst.charmap.subfont_id.clone_from(&src.charmap.subfont_id);

    dst.font_name.clone_from(&src.font_name);
    dst.enc_name.clone_from(&src.enc_name);

    dst.opt.slant = src.opt.slant;
    dst.opt.extend = src.opt.extend;
    dst.opt.bold = src.opt.bold;

    dst.opt.flags = src.opt.flags;
    dst.opt.mapc = src.opt.mapc;

    dst.opt.tounicode.clone_from(&src.opt.tounicode);
    dst.opt.otl_tags.clone_from(&src.opt.otl_tags);
    dst.opt.index = src.opt.index;
    dst.opt.charcoll.clone_from(&src.opt.charcoll);
    dst.opt.style = src.opt.style;
    dst.opt.stemv = src.opt.stemv;
    dst.opt.use_glyph_encoding = src.opt.use_glyph_encoding;
}

/// `strstr(s, pat) != NULL`.
fn contains(s: &[u8], pat: &[u8]) -> bool {
    s.windows(pat.len()).any(|w| w == pat)
}

/// A C string: the bytes before the first NUL.
fn cstr(s: &[u8]) -> &[u8] {
    match s.iter().position(|&c| c == 0) {
        Some(n) => &s[..n],
        None => s,
    }
}

/// `*p` where `p` may be at C's `endptr` (or past it): the NUL there.
fn at(s: &[u8], p: usize) -> u8 {
    s.get(p).copied().unwrap_or(0)
}

/// `fill_in_defaults` (static).
fn fill_in_defaults(mrec: &mut FontmapRec, tex_name: &[u8]) {
    if mrec
        .enc_name
        .as_deref()
        .is_some_and(|e| e == b"default" || e == b"none")
    {
        mrec.enc_name = None;
    }
    if mrec
        .font_name
        .as_deref()
        .is_some_and(|e| e == b"default" || e == b"none")
    {
        mrec.font_name = None;
    }
    // We *must* fill font_name either explicitly or by default.
    if mrec.font_name.is_none() {
        mrec.font_name = Some(tex_name.to_vec());
    }

    mrec.map_name = Some(tex_name.to_vec());

    // Use "UCS" character collection for Unicode SFD and Identity CMap
    // combination. For backward compatibility.
    if let (Some(sfd), Some(enc)) = (&mrec.charmap.sfd_name, &mrec.enc_name)
        && mrec.opt.charcoll.is_none()
        && (enc.as_slice() == b"Identity-H" || enc.as_slice() == b"Identity-V")
        && (contains(sfd, b"Uni")
            || contains(sfd, b"UBig")
            || contains(sfd, b"UBg")
            || contains(sfd, b"UGB")
            || contains(sfd, b"UKS")
            || contains(sfd, b"UJIS"))
    {
        mrec.opt.charcoll = Some(b"UCS".to_vec());
    }
}

/// `readline` (static): reads a line into `buf` (C's `work_buffer`, of
/// `buf.len()` bytes, NUL-terminated; what is past the NUL is left from
/// earlier lines, as in C) and cuts it at `%`. False at the end.
fn readline(buf: &mut [u8], fp: &mut MemFile) -> bool {
    let Some(line) = fp.mfgets(buf.len()) else {
        return false;
    };
    let n = line.len();
    buf[..n].copy_from_slice(&line);
    buf[n] = 0;
    // strchr(p, '%') within the C string
    let len = cstr(buf).len();
    if let Some(q) = buf[..len].iter().position(|&c| c == b'%') {
        buf[q] = 0;
    }
    true
}

/// `ISBLANK`.
fn is_blank(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

/// `skip_blank` (static).
fn skip_blank(s: &[u8], pp: &mut usize) {
    let mut p = *pp;
    if p >= s.len() {
        return;
    }
    while p < s.len() && is_blank(s[p]) {
        p += 1;
    }
    *pp = p;
}

/// `parse_string_value` (static). The value is a C string: it ends at a
/// NUL.
fn parse_string_value(s: &[u8], pp: &mut usize) -> Option<Vec<u8>> {
    let mut p = *pp;
    if p >= s.len() {
        return None;
    }
    let q;
    if s[p] == b'"' {
        q = crate::dpxutil::parse_c_string(s, &mut p);
    } else {
        let mut n = 0;
        while p < s.len() && !crate::fmt::is_c_space(s[p]) {
            p += 1;
            n += 1;
        }
        if n == 0 {
            return None;
        }
        q = Some(cstr(&s[*pp..*pp + n]).to_vec());
    }
    *pp = p;
    q
}

/// `ISDIGIT_WB`.
fn isdigit_wb(c: u8, b: i32) -> bool {
    let c = i32::from(c);
    (b <= 10 && c >= i32::from(b'0') && c < i32::from(b'0') + b)
        || (b > 10
            && ((c >= i32::from(b'0') && c <= i32::from(b'9'))
                || (c >= i32::from(b'a') && c < i32::from(b'a') + (b - 10))
                || (c >= i32::from(b'A') && c < i32::from(b'A') + (b - 10))))
}

/// `parse_integer_value` (static): no preceding spaces allowed.
fn parse_integer_value(s: &[u8], pp: &mut usize, base: i32) -> Option<Vec<u8>> {
    let mut p = *pp;
    let mut has_sign = false;
    let mut has_prefix = false;
    let mut base = base;

    debug_assert!(base == 0 || (2..=36).contains(&base));

    if p >= s.len() {
        return None;
    }

    if s[p] == b'-' || s[p] == b'+' {
        p += 1;
        has_sign = true;
    }
    if (base == 0 || base == 16) && p + 2 <= s.len() && s[p] == b'0' && s[p + 1] == b'x' {
        p += 2;
        has_prefix = true;
    }
    if base == 0 {
        if has_prefix {
            base = 16;
        } else if p < s.len() && s[p] == b'0' {
            base = 8;
        } else {
            base = 10;
        }
    }
    let mut n = 0;
    while p < s.len() && isdigit_wb(s[p], base) {
        p += 1;
        n += 1;
    }
    if n == 0 {
        return None;
    }
    if has_sign {
        n += 1;
    }
    if has_prefix {
        n += 2;
    }

    let q = s[*pp..*pp + n].to_vec();
    *pp = p;
    Some(q)
}

/// C's `strtol(q, NULL, base)` as an `int`.
fn strtol_int(q: &[u8], base: u32) -> i32 {
    crate::fmt::strtol(q, base).0 as i32
}

/// `fontmap_parse_mapdef_dpm` (static): dvipdfm format; 0 or -1.
fn fontmap_parse_mapdef_dpm(mrec: &mut FontmapRec, mapdef: &[u8]) -> i32 {
    let s = mapdef;
    let endptr = s.len();
    let mut p = 0usize;

    skip_blank(s, &mut p);
    // encoding field
    if p < endptr && s[p] != b'-' {
        mrec.enc_name = parse_string_value(s, &mut p);
        skip_blank(s, &mut p);
    }

    // fontname or font filename field
    if p < endptr && s[p] != b'-' {
        mrec.font_name = parse_string_value(s, &mut p);
        skip_blank(s, &mut p);
    }
    if let Some(font_name) = mrec.font_name.clone() {
        // Several options are encoded in font_name for compatibility
        // with dvipdfm.
        let tmp = strip_options(&font_name, &mut mrec.opt);
        if tmp.is_some() {
            mrec.font_name = tmp;
        }
    }

    skip_blank(s, &mut p);
    // Parse any remaining arguments
    while p + 1 < endptr && s[p] != b'\r' && s[p] != b'\n' && s[p] == b'-' {
        let mopt = s[p + 1];

        p += 2;
        skip_blank(s, &mut p);
        match mopt {
            b's' => {
                // Slant option
                let Some(q) = crate::dpxutil::parse_float_decimal(s, &mut p) else {
                    warn!("Missing a number value for 's' option.");
                    return -1;
                };
                mrec.opt.slant = crate::fmt::atof(&q);
            }
            b'e' => {
                // Extend option
                let Some(q) = crate::dpxutil::parse_float_decimal(s, &mut p) else {
                    warn!("Missing a number value for 'e' option.");
                    return -1;
                };
                mrec.opt.extend = crate::fmt::atof(&q);
                if mrec.opt.extend <= 0.0 {
                    warn!("Invalid value for 'e' option");
                    return -1;
                }
            }
            b'b' => {
                // Fake-bold option
                let Some(q) = crate::dpxutil::parse_float_decimal(s, &mut p) else {
                    warn!("Missing a number value for 'b' option.");
                    return -1;
                };
                mrec.opt.bold = crate::fmt::atof(&q);
                if mrec.opt.bold <= 0.0 {
                    warn!("Invalid value for 'b' option");
                    return -1;
                }
            }
            b'r' => {
                // Remap option; obsolete; just ignore
            }
            b'i' => {
                // TTC index
                let Some(q) = parse_integer_value(s, &mut p, 10) else {
                    warn!("Missing TTC index number...");
                    return -1;
                };
                mrec.opt.index = crate::fmt::strtol(&q, 10).0 as u32;
            }
            b'p' => {
                // UCS plane: just for testing
                let Some(q) = parse_integer_value(s, &mut p, 0) else {
                    warn!("Missing a number for 'p' option.");
                    return -1;
                };
                let v = strtol_int(&q, 0);
                if !(0..=16).contains(&v) {
                    warn!("Invalid value for option 'p'");
                } else {
                    mrec.opt.mapc = v << 16;
                }
            }
            b'u' => {
                // ToUnicode
                let q = parse_string_value(s, &mut p);
                if q.is_some() {
                    mrec.opt.tounicode = q;
                } else {
                    warn!("Missing string value for option 'u'.");
                    return -1;
                }
            }
            b'v' => {
                // StemV
                let Some(q) = parse_integer_value(s, &mut p, 10) else {
                    warn!("Missing a number for 'v' option.");
                    return -1;
                };
                mrec.opt.stemv = strtol_int(&q, 0);
            }
            b'l' => {
                let q = parse_string_value(s, &mut p);
                if q.is_some() {
                    mrec.opt.otl_tags = q;
                } else {
                    warn!("Missing string value for option 'l'.");
                    return -1;
                }
            }
            b'm' => {
                // Map single bytes char 0xab to double byte char 0xcdab
                if p + 4 <= endptr && s[p] == b'<' && s[p + 3] == b'>' {
                    p += 1;
                    let Some(q) = parse_integer_value(s, &mut p, 16) else {
                        warn!("Invalid value for option 'm'.");
                        return -1;
                    };
                    if p < endptr && s[p] != b'>' {
                        warn!("Invalid value for option 'm'");
                        return -1;
                    }
                    let v = strtol_int(&q, 16);
                    mrec.opt.mapc = (v << 8) & 0x0000_ff00;
                    p += 1;
                } else if p + 4 <= endptr && &s[p..p + 4] == b"sfd:" {
                    // SFD mapping: sfd:Big5,00
                    p += 4;
                    skip_blank(s, &mut p);
                    let Some(q) = parse_string_value(s, &mut p) else {
                        warn!("Missing value for option 'm'.");
                        return -1;
                    };
                    let Some(r) = q.iter().position(|&c| c == b',') else {
                        warn!("Invalid value for option 'm'");
                        return -1;
                    };
                    let rest = &q[r + 1..];
                    let mut rr = 0;
                    skip_blank(rest, &mut rr);
                    if rr >= rest.len() {
                        warn!("Invalid value for option 'm'");
                        return -1;
                    }
                    mrec.charmap.sfd_name = Some(q[..r].to_vec());
                    mrec.charmap.subfont_id = Some(rest[rr..].to_vec());
                } else if p + 4 < endptr && &s[p..p + 4] == b"pad:" {
                    p += 4;
                    skip_blank(s, &mut p);
                    let Some(q) = parse_integer_value(s, &mut p, 16) else {
                        warn!("Invalid value for option 'm'.");
                        return -1;
                    };
                    if p < endptr && !crate::fmt::is_c_space(s[p]) {
                        warn!("Invalid value for option 'm'");
                        return -1;
                    }
                    let v = strtol_int(&q, 16);
                    mrec.opt.mapc = (v << 8) & 0x0000_ff00;
                } else {
                    warn!("Invalid value for option 'm'.");
                    return -1;
                }
            }
            b'w' => {
                // Writing mode (for unicode encoding)
                if mrec.enc_name.as_deref() != Some(b"unicode".as_slice()) {
                    warn!("Fontmap option 'w' meaningless for encoding other than \"unicode\".");
                    return -1;
                }
                let Some(q) = parse_integer_value(s, &mut p, 10) else {
                    warn!("Missing wmode value...");
                    return -1;
                };
                let v = crate::fmt::atoi(&q) as i32;
                if v == 1 {
                    mrec.opt.flags |= FONTMAP_OPT_VERT;
                } else if v == 0 {
                    mrec.opt.flags &= !FONTMAP_OPT_VERT;
                } else {
                    warn!("Invalid value for option 'w'");
                }
            }
            _ => {
                warn!("Unrecognized font map option: '{}'", mopt as char);
                return -1;
            }
        }
        skip_blank(s, &mut p);
    }

    if p < endptr && s[p] != b'\r' && s[p] != b'\n' {
        warn!("Invalid char in fontmap line: {}", s[p] as char);
        return -1;
    }

    0
}

/// `fontmap_parse_mapdef_dps` (static): dvips/pdfTeX format; 0 or -1.
fn fontmap_parse_mapdef_dps(mrec: &mut FontmapRec, mapdef: &[u8]) -> i32 {
    let s = mapdef;
    let endptr = s.len();
    let mut p = 0usize;

    skip_blank(s, &mut p);

    // The first field (after TFM name) must be PostScript name.
    // However, pdftex.map allows a line without PostScript name.
    if at(s, p) != b'"' && at(s, p) != b'<' {
        if p < endptr {
            let _ = parse_string_value(s, &mut p);
            skip_blank(s, &mut p);
        } else {
            warn!("Missing a PostScript font name.");
            return -1;
        }
    }

    if p >= endptr {
        return 0;
    }

    // Parse any remaining arguments
    while p < endptr && s[p] != b'\r' && s[p] != b'\n' && (s[p] == b'<' || s[p] == b'"') {
        match s[p] {
            b'<' => {
                // encoding or fontfile field
                // If we see <[ or <<, just ignore the second char instead
                // of doing as directed (define encoding file, fully embed).
                p += 1;
                if p < endptr && (s[p] == b'[' || s[p] == b'<') {
                    p += 1;
                }
                skip_blank(s, &mut p);
                if let Some(q) = parse_string_value(s, &mut p) {
                    let n = q.len();
                    if n > 4 && &q[n - 4..] == b".enc" {
                        mrec.enc_name = Some(q);
                    } else {
                        mrec.font_name = Some(q);
                    }
                }
                skip_blank(s, &mut p);
            }
            b'"' => {
                // Options
                if let Some(q) = parse_string_value(s, &mut p) {
                    let e = q.len();
                    let mut r = 0usize;
                    skip_blank(&q, &mut r);
                    while r < e {
                        if let Some(sv) = crate::dpxutil::parse_float_decimal(&q, &mut r) {
                            skip_blank(&q, &mut r);
                            if let Some(t) = parse_string_value(&q, &mut r) {
                                if t.as_slice() == b"SlantFont" {
                                    mrec.opt.slant = crate::fmt::atof(&sv);
                                } else if t.as_slice() == b"ExtendFont" {
                                    mrec.opt.extend = crate::fmt::atof(&sv);
                                }
                            }
                        } else if parse_string_value(&q, &mut r).is_some() {
                            // skip
                        }
                        skip_blank(&q, &mut r);
                    }
                }
                skip_blank(s, &mut p);
            }
            _ => {
                warn!("Found an invalid entry");
                return -1;
            }
        }
        skip_blank(s, &mut p);
    }

    if p < endptr && s[p] != b'\r' && s[p] != b'\n' {
        warn!("Invalid char in fontmap line: {}", s[p] as char);
        return -1;
    }

    0
}

/// `fontmap_invalid`.
fn fontmap_invalid(m: &FontmapRec) -> bool {
    m.map_name.is_none() || m.font_name.is_none()
}

/// `chop_sfd_name` (static): the font name without `@SFD@` and the SFD
/// name, or none.
fn chop_sfd_name(tex_name: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let m = tex_name.iter().position(|&c| c == b'@')?;
    if m + 1 >= tex_name.len() || m == 0 {
        return None;
    }
    let p = m + 1;
    let n = tex_name[p..].iter().position(|&c| c == b'@')?;
    if n == 0 {
        return None;
    }
    let q = p + n + 1;

    let mut fontname = tex_name[..m].to_vec();
    fontname.extend_from_slice(&tex_name[q..]);
    let sfd_name = tex_name[p..p + n].to_vec();
    Some((fontname, sfd_name))
}

/// `make_subfont_name` (static).
fn make_subfont_name(map_name: &[u8], sfd_name: &[u8], sub_id: &[u8]) -> Option<Vec<u8>> {
    let m = map_name.iter().position(|&c| c == b'@')?;
    if m == 0 {
        return None;
    }
    let qoff = map_name[m + 1..].iter().position(|&c| c == b'@')?;
    if qoff == 0 {
        return None;
    }
    let q = m + 1 + qoff;
    let n = q - m + 1; // including two '@'
    if sfd_name.len() != n - 2 || map_name[m + 1..m + 1 + (n - 2)] != *sfd_name {
        return None;
    }
    let mut tfm_name = map_name[..m].to_vec();
    tfm_name.extend_from_slice(sub_id);
    if q + 1 < map_name.len() {
        // not ending with '@'
        tfm_name.extend_from_slice(&map_name[q + 1..]);
    }
    Some(tfm_name)
}

/// `is_pdfm_mapline`: -1 dvips/pdfTeX format, 1 dvipdfm format, 0 either
/// (two entries); see C. `mline` is a C string (it ends at a NUL).
#[must_use]
pub fn is_pdfm_mapline(mline: &[u8]) -> i32 {
    let s = cstr(mline);
    let mut n = 0;

    if s.contains(&b'"') || s.contains(&b'<') {
        return -1; // DVIPS/pdfTeX format
    }

    let mut p = 0usize;
    let endptr = s.len();

    skip_blank(s, &mut p);

    while p < endptr {
        // Break if '-' preceeded by blanks is found. (DVIPDFM format)
        if s[p] == b'-' {
            return 1;
        }
        n += 1;
        while p < endptr && !is_blank(s[p]) {
            p += 1;
        }
        skip_blank(s, &mut p);
    }

    // Two entries: TFM_NAME PS_NAME only (DVIPS format)
    // Otherwise (DVIPDFM format)
    if n == 2 { 0 } else { 1 }
}

/// `pdf_read_fontmap_line`: `mline` is the whole line (C's
/// `mline_strlen`); `format` > 0 dvipdfm, else dvips. 0 or -1.
pub fn pdf_read_fontmap_line(mrec: &mut FontmapRec, mline: &[u8], format: i32) -> i32 {
    let s = mline;
    let mut p = 0usize;

    skip_blank(s, &mut p);
    if p >= s.len() {
        return -1;
    }

    let Some(q) = parse_string_value(s, &mut p) else {
        return -1;
    };

    let error = if format > 0 {
        // DVIPDFM format
        fontmap_parse_mapdef_dpm(mrec, &s[p..])
    } else {
        // DVIPS/pdfTeX format
        fontmap_parse_mapdef_dps(mrec, &s[p..])
    };
    if error == 0 {
        if let Some((fnt_name, sfd_name)) = chop_sfd_name(&q) {
            if mrec.font_name.is_none() {
                // In the case of subfonts, the base name (before the
                // character '@') will be used as a font_name by default.
                mrec.font_name = Some(fnt_name);
            }
            mrec.charmap.sfd_name = Some(sfd_name);
        }
        fill_in_defaults(mrec, &q);
    }

    error
}

/// `substr` (static): the bytes before `stop`, advancing `*pp` past it.
fn substr(s: &[u8], pp: &mut usize, stop: u8) -> Option<Vec<u8>> {
    let off = s[*pp..].iter().position(|&c| c == stop)?;
    if off == 0 {
        return None;
    }
    let sstr = s[*pp..*pp + off].to_vec();
    *pp += off + 1;
    Some(sstr)
}

/// `strip_options` (static): the font name without `:n:`, `!`, `/csi`,
/// `,Bold` options, which go to `opt`.
fn strip_options(map_name: &[u8], opt: &mut FontmapOpt) -> Option<Vec<u8>> {
    let s = cstr(map_name);
    let mut p = 0usize;
    let font_name;
    let mut have_csi = false;
    let mut have_style = false;

    opt.charcoll = None;
    opt.index = 0;
    opt.style = FONTMAP_STYLE_NONE;
    opt.flags = 0;

    if at(s, p) == b':' && at(s, p + 1).is_ascii_digit() {
        let (v, n) = crate::fmt::strtol(&s[p + 1..], 10);
        opt.index = v as u32;
        let next = p + 1 + n;
        if at(s, next) == b':' {
            p = next + 1;
        } else {
            opt.index = 0;
        }
    }
    if at(s, p) == b'!' {
        // no-embedding
        p += 1;
        if at(s, p) == 0 {
            error!("Invalid map record: {:?} (--> {:?})", map_name, &s[p..]);
        }
        opt.flags |= FONTMAP_OPT_NOEMBED;
    }

    if let Some(off) = s[p..].iter().position(|&c| c == CID_MAPREC_CSI_DELIM) {
        if off == 0 {
            error!("Invalid map record: {:?} (--> {:?})", map_name, &s[p..]);
        }
        font_name = substr(s, &mut p, CID_MAPREC_CSI_DELIM);
        have_csi = true;
    } else if let Some(off) = s[p..].iter().position(|&c| c == b',') {
        if off == 0 {
            error!("Invalid map record: {:?} (--> {:?})", map_name, &s[p..]);
        }
        font_name = substr(s, &mut p, b',');
        have_style = true;
    } else {
        font_name = Some(s[p..].to_vec());
    }

    if have_csi {
        if s[p..].contains(&b',') {
            opt.charcoll = substr(s, &mut p, b',');
            have_style = true;
        } else if at(s, p) == 0 {
            error!("Invalid map record: {:?}.", map_name);
        } else {
            opt.charcoll = Some(s[p..].to_vec());
        }
    }

    if have_style {
        let r = &s[p..];
        if r.starts_with(b"BoldItalic") {
            if r.len() > 10 {
                error!("Invalid map record: {:?} (--> {:?})", map_name, r);
            }
            opt.style = FONTMAP_STYLE_BOLDITALIC;
        } else if r.starts_with(b"Bold") {
            if r.len() > 4 {
                error!("Invalid map record: {:?} (--> {:?})", map_name, r);
            }
            opt.style = FONTMAP_STYLE_BOLD;
        } else if r.starts_with(b"Italic") {
            if r.len() > 6 {
                error!("Invalid map record: {:?} (--> {:?})", map_name, r);
            }
            opt.style = FONTMAP_STYLE_ITALIC;
        }
    }

    font_name
}

/// C's `work_buffer` size (mfileio.h's `WORK_BUFFER_SIZE`).
const WORK_BUFFER_SIZE: usize = 1024;

impl Dpx {
    /// `pdf_init_fontmaps`.
    pub fn pdf_init_fontmaps(&mut self) {
        self.fontmap.fontmap = Some(HtTable::ht_init_table());
    }

    /// `pdf_close_fontmaps`. (C then calls subfont.c's
    /// `release_sfd_record`, which has nothing to release here: SFD
    /// files are never loaded.)
    pub fn pdf_close_fontmaps(&mut self) {
        if let Some(t) = self.fontmap.fontmap.as_mut() {
            t.ht_clear_table();
        }
        self.fontmap.fontmap = None;
    }

    fn fontmap_table(&mut self) -> &mut HtTable<FontmapRec> {
        self.fontmap
            .fontmap
            .as_mut()
            .expect("fontmap not initialized")
    }

    /// `pdf_load_fontmap_file`: `mode` is a `FONTMAP_RMODE_*`; 0 or -1.
    /// The lines are read into a buffer that keeps, past each line's
    /// end, what earlier lines left (C's `work_buffer`, which C's
    /// parsers may read past the NUL for a line with leading blanks).
    pub fn pdf_load_fontmap_file(&mut self, filename: &[u8], mode: i32) -> i32 {
        let mut lpos = 0;
        let mut error = 0;
        let mut format = 0;

        assert!(self.fontmap.fontmap.is_some());

        let Some(mut fp) = self.dpx_open_file(filename, crate::dpxfile::ResType::Fontmap) else {
            warn!("Couldn't open font map file.");
            return -1;
        };

        let mut work_buffer = vec![0u8; WORK_BUFFER_SIZE];
        while error == 0 && readline(&mut work_buffer, &mut fp) {
            lpos += 1;
            let llen = cstr(&work_buffer).len();
            let endptr = llen;
            let mut p = 0usize;

            skip_blank(&work_buffer[..endptr], &mut p);
            if p == endptr {
                continue;
            }

            let m = is_pdfm_mapline(&work_buffer[p..]);

            if format * m < 0 {
                // mismatch
                warn!("Found a mismatched fontmap line {} from file.", lpos);
                continue;
            }
            format += m;

            let mut mrec = FontmapRec::default();

            // format > 0: DVIPDFM, format <= 0: DVIPS/pdfTeX
            let end = (p + llen).min(work_buffer.len());
            error = pdf_read_fontmap_line(&mut mrec, &work_buffer[p..end], format);
            if error != 0 {
                warn!("Invalid map record in fontmap line {} from file.", lpos);
                continue;
            }
            let kp = mrec.map_name.clone().unwrap_or_default();
            match mode {
                FONTMAP_RMODE_REPLACE => {
                    self.pdf_insert_fontmap_record(&kp, &mrec);
                }
                FONTMAP_RMODE_APPEND => {
                    self.pdf_append_fontmap_record(&kp, &mrec);
                }
                FONTMAP_RMODE_REMOVE => {
                    self.pdf_remove_fontmap_record(&kp);
                }
                _ => {}
            }
        }

        error
    }

    /// `pdf_append_fontmap_record`: 0 or -1.
    pub fn pdf_append_fontmap_record(&mut self, kp: &[u8], mrec: &FontmapRec) -> i32 {
        if fontmap_invalid(mrec) {
            warn!("Invalid fontmap record...");
            return -1;
        }

        if let Some((_fnt_name, _sfd_name)) = chop_sfd_name(kp) {
            todo!("SFD subfonts (subfont.c's sfd_get_subfont_ids) are not ported");
        }

        let t = self.fontmap_table();
        if t.ht_lookup_table(kp).is_none() {
            let mut m = FontmapRec::default();
            pdf_copy_fontmap_record(&mut m, mrec);
            if m.map_name.as_deref() == Some(kp) {
                m.map_name = None;
            }
            t.ht_insert_table(kp, m);
        }

        0
    }

    /// `pdf_remove_fontmap_record`: 0 or -1.
    pub fn pdf_remove_fontmap_record(&mut self, kp: &[u8]) -> i32 {
        if let Some((_fnt_name, _sfd_name)) = chop_sfd_name(kp) {
            todo!("SFD subfonts (subfont.c's sfd_get_subfont_ids) are not ported");
        }

        self.fontmap_table().ht_remove_table(kp);

        0
    }

    /// `pdf_insert_fontmap_record`: a copy of the record inserted, or none.
    pub fn pdf_insert_fontmap_record(
        &mut self,
        kp: &[u8],
        mrec: &FontmapRec,
    ) -> Option<FontmapRec> {
        if fontmap_invalid(mrec) {
            warn!("Invalid fontmap record...");
            return None;
        }

        if let Some((_fnt_name, _sfd_name)) = chop_sfd_name(kp) {
            todo!("SFD subfonts (subfont.c's sfd_get_subfont_ids) are not ported");
        }

        let mut m = FontmapRec::default();
        pdf_copy_fontmap_record(&mut m, mrec);
        if m.map_name.as_deref() == Some(kp) {
            m.map_name = None;
        }
        let ret = m.clone();
        self.fontmap_table().ht_insert_table(kp, m);

        Some(ret)
    }

    /// `pdf_lookup_fontmap_record`: a copy of the record.
    pub fn pdf_lookup_fontmap_record(&mut self, kp: &[u8]) -> Option<FontmapRec> {
        self.fontmap
            .fontmap
            .as_ref()
            .and_then(|t| t.ht_lookup_table(kp).cloned())
    }

    /// `pdf_insert_native_fontmap_record`: a copy of the record inserted.
    /// The key is `path/index/H|V/extend/slant/embolden` (C's
    /// `"%s/%d/%c/%d/%d/%d"`).
    pub fn pdf_insert_native_fontmap_record(
        &mut self,
        filename: &[u8],
        index: u32,
        layout_dir: i32,
        extend: i32,
        slant: i32,
        embolden: i32,
    ) -> Option<FontmapRec> {
        let mut fontmap_key = filename.to_vec();
        fontmap_key.extend_from_slice(
            format!(
                "/{}/{}/{}/{}/{}",
                index as i32,
                if layout_dir == 0 { 'H' } else { 'V' },
                extend,
                slant,
                embolden
            )
            .as_bytes(),
        );

        let mut mrec = FontmapRec::default();

        mrec.map_name = Some(fontmap_key.clone());
        mrec.enc_name = Some(if layout_dir == 0 {
            b"Identity-H".to_vec()
        } else {
            b"Identity-V".to_vec()
        });
        mrec.font_name = Some(filename.to_vec());
        mrec.opt.index = index;
        if layout_dir != 0 {
            mrec.opt.flags |= FONTMAP_OPT_VERT;
        }

        fill_in_defaults(&mut mrec, &fontmap_key);

        mrec.opt.extend = f64::from(extend) / 65536.0;
        mrec.opt.slant = f64::from(slant) / 65536.0;
        mrec.opt.bold = f64::from(embolden) / 65536.0;
        mrec.opt.use_glyph_encoding = 1;

        let kp = mrec.map_name.clone().unwrap_or_default();
        self.pdf_insert_fontmap_record(&kp, &mrec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapline_kinds() {
        assert_eq!(is_pdfm_mapline(b"cmr10 CMR10 <cmr10.pfb"), -1);
        assert_eq!(
            is_pdfm_mapline(b"ptmr8r Times-Roman \"TeXBase1Encoding ReEncodeFont\" <8r.enc"),
            -1
        );
        assert_eq!(is_pdfm_mapline(b"cmr10 CMR10"), 0);
        assert_eq!(is_pdfm_mapline(b"rml H Ryumin-Light"), 1);
        assert_eq!(is_pdfm_mapline(b"foo default bar -e 1.2"), 1);
        assert_eq!(is_pdfm_mapline(b"foo"), 1);
    }

    #[test]
    fn strip() {
        let mut opt = FontmapOpt::default();
        let n = strip_options(b":1:!msmincho/AJ16,Bold", &mut opt);
        assert_eq!(n.as_deref(), Some(b"msmincho".as_slice()));
        assert_eq!(opt.index, 1);
        assert_eq!(opt.flags, FONTMAP_OPT_NOEMBED);
        assert_eq!(opt.charcoll.as_deref(), Some(b"AJ16".as_slice()));
        assert_eq!(opt.style, FONTMAP_STYLE_BOLD);

        let mut opt = FontmapOpt::default();
        let n = strip_options(b":x:foo,Italic", &mut opt);
        assert_eq!(n.as_deref(), Some(b":x:foo".as_slice()));
        assert_eq!(opt.style, FONTMAP_STYLE_ITALIC);

        let mut opt = FontmapOpt::default();
        let n = strip_options(b":2foo", &mut opt);
        assert_eq!(n.as_deref(), Some(b":2foo".as_slice()));
        assert_eq!(opt.index, 0);
    }

    #[test]
    fn subfont_names() {
        assert_eq!(
            chop_sfd_name(b"gbk@Unicode@x"),
            Some((b"gbkx".to_vec(), b"Unicode".to_vec()))
        );
        assert_eq!(chop_sfd_name(b"@a@"), None);
        assert_eq!(chop_sfd_name(b"a@@"), None);
        assert_eq!(
            make_subfont_name(b"gbk@Unicode@", b"Unicode", b"01"),
            Some(b"gbk01".to_vec())
        );
        assert_eq!(make_subfont_name(b"gbk@Uni@", b"Unicode", b"01"), None);
    }

    #[test]
    fn parse_integers() {
        let mut p = 0;
        assert_eq!(
            parse_integer_value(b"0x1fz", &mut p, 0).as_deref(),
            Some(b"0x1f".as_slice())
        );
        assert_eq!(p, 4);
        let mut p = 0;
        assert_eq!(
            parse_integer_value(b"-019", &mut p, 0).as_deref(),
            Some(b"-01".as_slice())
        );
        let mut p = 0;
        assert_eq!(parse_integer_value(b"z", &mut p, 10), None);
    }
}

#[cfg(test)]
mod line_tests {
    use super::*;

    #[test]
    fn dps_line() {
        // (quoted options need dpxutil's parse_c_string)
        let mut m = FontmapRec::default();
        let line = b"pplr8r Palatino-Roman <8r.enc <uplr8a.pfb";
        assert_eq!(pdf_read_fontmap_line(&mut m, line, -1), 0);
        assert_eq!(m.map_name.as_deref(), Some(b"pplr8r".as_slice()));
        assert_eq!(m.enc_name.as_deref(), Some(b"8r.enc".as_slice()));
        assert_eq!(m.font_name.as_deref(), Some(b"uplr8a.pfb".as_slice()));

        let mut m = FontmapRec::default();
        assert_eq!(pdf_read_fontmap_line(&mut m, b"cmr10 CMR10", 0), 0);
        assert_eq!(m.font_name.as_deref(), Some(b"cmr10".as_slice()));
        assert_eq!(m.enc_name, None);

        let mut m = FontmapRec::default();
        assert_eq!(pdf_read_fontmap_line(&mut m, b"cmr10", 0), -1);
    }

    #[test]
    fn dpm_line() {
        let mut m = FontmapRec::default();
        let line = b"rml  H :1:!Ryumin-Light/AJ1-2,Bold -i 3 -v 010 -m <0a>";
        assert_eq!(pdf_read_fontmap_line(&mut m, line, 1), 0);
        assert_eq!(m.enc_name.as_deref(), Some(b"H".as_slice()));
        assert_eq!(m.font_name.as_deref(), Some(b"Ryumin-Light".as_slice()));
        assert_eq!(m.opt.charcoll.as_deref(), Some(b"AJ1-2".as_slice()));
        assert_eq!(m.opt.style, FONTMAP_STYLE_BOLD);
        assert_eq!(m.opt.flags, FONTMAP_OPT_NOEMBED);
        assert_eq!(m.opt.index, 3);
        assert_eq!(m.opt.stemv, 8);
        assert_eq!(m.opt.mapc, 0x0a00);

        let mut m = FontmapRec::default();
        assert_eq!(
            pdf_read_fontmap_line(&mut m, b"foo unicode bar -w 1 -p 2", 1),
            0
        );
        assert_eq!(m.opt.flags, FONTMAP_OPT_VERT);
        assert_eq!(m.opt.mapc, 2 << 16);
        let mut m = FontmapRec::default();
        assert_eq!(pdf_read_fontmap_line(&mut m, b"foo H bar -w 1", 1), -1);
        let mut m = FontmapRec::default();
        assert_eq!(pdf_read_fontmap_line(&mut m, b"foo H bar -q", 1), -1);
        let mut m = FontmapRec::default();
        assert_eq!(
            pdf_read_fontmap_line(&mut m, b"foo H bar -m sfd:Big5,00", 1),
            0
        );
        assert_eq!(m.charmap.subfont_id.as_deref(), Some(b"00".as_slice()));
        let mut m = FontmapRec::default();
        assert_eq!(
            pdf_read_fontmap_line(&mut m, b"foo H bar -m sfd:Big5, 00", 1),
            -1
        );
        let mut m = FontmapRec::default();
        assert_eq!(
            pdf_read_fontmap_line(&mut m, b"foo H bar -m sfd:Big5,00", 1),
            0
        );
        assert_eq!(m.charmap.sfd_name.as_deref(), Some(b"Big5".as_slice()));
    }
}
