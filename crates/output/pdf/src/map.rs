//! Font map files (`mapfile.c`): which PostScript font, font file and
//! encoding a TFM name stands for.

use std::collections::BTreeMap;

/// `F_INCLUDED` and friends (`ptexmac.h`): what a map entry's font file
/// is and how it is embedded.
pub mod flags {
    /// The font file is embedded.
    pub const INCLUDED: u8 = 0x01;
    /// The embedded font is subset.
    pub const SUBSETTED: u8 = 0x02;
    /// One of the 14 standard fonts.
    pub const STD_T1FONT: u8 = 0x04;
    /// A Type 1 font.
    pub const TYPE1: u8 = 0x10;
    /// A TrueType font.
    pub const TRUETYPE: u8 = 0x20;
    /// An OpenType font.
    pub const OTF: u8 = 0x40;
    /// A bitmap PK font.
    pub const PK: u8 = 0x80;
}

/// One map entry (`fm_entry`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MapEntry {
    /// `tfm_name`.
    pub tfm_name: Vec<u8>,
    /// `ps_name`.
    pub ps_name: Option<Vec<u8>>,
    /// `fd_flags`, when the line gives them.
    pub fd_flags: Option<i32>,
    /// `ff_name`: the font file.
    pub ff_name: Option<Vec<u8>>,
    /// `encname`: the `.enc` file.
    pub encname: Option<Vec<u8>>,
    /// The [`flags`].
    pub kind: u8,
    /// `SlantFont`, in thousandths.
    pub slant: i32,
    /// `ExtendFont`, in thousandths.
    pub extend: i32,
}

impl MapEntry {
    /// `is_included`.
    #[must_use]
    pub fn is_included(&self) -> bool {
        self.kind & flags::INCLUDED != 0
    }
    /// `is_subsetted`.
    #[must_use]
    pub fn is_subsetted(&self) -> bool {
        self.kind & flags::SUBSETTED != 0
    }
    /// `is_type1`.
    #[must_use]
    pub fn is_type1(&self) -> bool {
        self.kind & flags::TYPE1 != 0
    }
    /// `is_truetype`.
    #[must_use]
    pub fn is_truetype(&self) -> bool {
        self.kind & flags::TRUETYPE != 0
    }
    /// `is_opentype`.
    #[must_use]
    pub fn is_opentype(&self) -> bool {
        self.kind & flags::OTF != 0
    }
    /// `is_pk`.
    #[must_use]
    pub fn is_pk(&self) -> bool {
        self.kind & flags::PK != 0
    }
    /// `is_std_t1font`.
    #[must_use]
    pub fn is_std_t1font(&self) -> bool {
        self.kind & flags::STD_T1FONT != 0
    }
    /// `is_fontfile`.
    #[must_use]
    pub fn is_fontfile(&self) -> bool {
        self.ff_name.is_some()
    }
    /// `is_reencoded`.
    #[must_use]
    pub fn is_reencoded(&self) -> bool {
        self.encname.is_some()
    }
}

/// `is_cfg_comment` (`ptexmac.h`).
fn is_cfg_comment(c: u8) -> bool {
    matches!(c, 10 | b'*' | b'#' | b';' | b'%')
}

/// `read_field` (`mapfile.c`): a field up to a blank, `<`, `"` or the end;
/// then one blank is skipped.
fn read_field(s: &[u8], r: &mut usize) -> Vec<u8> {
    let start = *r;
    while *r < s.len() && !matches!(s[*r], b' ' | b'<' | b'"') {
        *r += 1;
    }
    let f = s[start..*r].to_vec();
    skip(s, r, b' ');
    f
}

fn skip(s: &[u8], r: &mut usize, c: u8) {
    if s.get(*r) == Some(&c) {
        *r += 1;
    }
}

fn at(s: &[u8], r: usize) -> u8 {
    s.get(r).copied().unwrap_or(0)
}

/// `check_std_t1font` (`mapfile.c`): whether `s` is one of the 14
/// standard font names.
#[must_use]
pub fn is_std_t1font_name(s: &[u8]) -> bool {
    const NAMES: [&[u8]; 14] = [
        b"Courier",
        b"Courier-Bold",
        b"Courier-Oblique",
        b"Courier-BoldOblique",
        b"Helvetica",
        b"Helvetica-Bold",
        b"Helvetica-Oblique",
        b"Helvetica-BoldOblique",
        b"Symbol",
        b"Times-Roman",
        b"Times-Bold",
        b"Times-Italic",
        b"Times-BoldItalic",
        b"ZapfDingbats",
    ];
    NAMES.contains(&s)
}

/// A C `%f` conversion at the start of `s`: the number and the bytes it
/// took, if any.
fn scan_float(s: &[u8]) -> Option<(f32, usize)> {
    let mut i = 0;
    if matches!(at(s, i), b'+' | b'-') {
        i += 1;
    }
    let digits = |i: &mut usize| {
        let st = *i;
        while at(s, *i).is_ascii_digit() {
            *i += 1;
        }
        *i - st
    };
    let mut n = digits(&mut i);
    if at(s, i) == b'.' {
        i += 1;
        n += digits(&mut i);
    }
    if n == 0 {
        return None;
    }
    if matches!(at(s, i), b'e' | b'E') {
        let mut j = i + 1;
        if matches!(at(s, j), b'+' | b'-') {
            j += 1;
        }
        if at(s, j).is_ascii_digit() {
            i = j;
            digits(&mut i);
        }
    }
    let v: f32 = std::str::from_utf8(&s[..i]).ok()?.parse().ok()?;
    Some((v, i))
}

/// `fm_scan_line` (`mapfile.c`) for one line (without its newline): the
/// entry, or `None` for a comment, a blank or an invalid line.
#[must_use]
#[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
pub fn parse_line(s: &[u8]) -> Option<MapEntry> {
    let mut r = 0;
    if s.is_empty() || is_cfg_comment(s[0]) {
        return None;
    }
    let mut fm = MapEntry {
        tfm_name: read_field(s, &mut r),
        ..MapEntry::default()
    };
    let mut done = r >= s.len();
    if !done && !at(s, r).is_ascii_digit() {
        let f = read_field(s, &mut r);
        if !f.is_empty() {
            fm.ps_name = Some(f);
        }
        done = r >= s.len();
    }
    if !done {
        if at(s, r).is_ascii_digit() {
            let mut e = r;
            while at(s, e).is_ascii_digit() {
                e += 1;
            }
            if matches!(at(s, e), b' ' | b'"' | b'<' | 0) {
                fm.fd_flags = std::str::from_utf8(&s[r..e]).ok()?.parse().ok();
                r = e;
            }
        }
        let (mut u, mut v) = (0u8, 0u8);
        loop {
            skip(s, &mut r, b' ');
            match at(s, r) {
                0 => break,
                b'"' => {
                    r += 1;
                    u = 0;
                    v = 0;
                    loop {
                        skip(s, &mut r, b' ');
                        if let Some((mut d, j)) = scan_float(&s[r..]) {
                            let mut e = r + j;
                            while at(s, e) == b' ' {
                                e += 1;
                            }
                            if matches!(at(s, e - 1), b'E' | b'e') {
                                e -= 1;
                            }
                            let rest = &s[e..];
                            if rest.starts_with(b"SlantFont") {
                                d *= 1000.0;
                                fm.slant = if d > 0.0 { d + 0.5 } else { d - 0.5 } as i32;
                                r = e + 9;
                            } else if rest.starts_with(b"ExtendFont") {
                                d *= 1000.0;
                                fm.extend = if d > 0.0 { d + 0.5 } else { d - 0.5 } as i32;
                                if fm.extend == 1000 {
                                    fm.extend = 0;
                                }
                                r = e + 10;
                            } else {
                                r = e;
                                while !matches!(at(s, r), b' ' | b'"' | 0) {
                                    r += 1;
                                }
                            }
                        } else {
                            while !matches!(at(s, r), b' ' | b'"' | 0) {
                                r += 1;
                            }
                        }
                        if at(s, r) != b' ' {
                            break;
                        }
                    }
                    if at(s, r) == b'"' {
                        r += 1;
                    } else {
                        return None;
                    }
                }
                _ => {
                    let (mut a, mut b) = (0u8, 0u8);
                    if at(s, r) == b'<' {
                        a = b'<';
                        r += 1;
                        if matches!(at(s, r), b'<' | b'[') {
                            b = at(s, r);
                            r += 1;
                        }
                    }
                    let buf = read_field(s, &mut r);
                    if buf.len() > 4 && buf[buf.len() - 4..].eq_ignore_ascii_case(b".enc") {
                        fm.encname = Some(buf);
                        u = 0;
                        v = 0;
                    } else if !buf.is_empty() {
                        if a == b'<' || u == b'<' {
                            fm.kind |= flags::INCLUDED;
                            if (a == b'<' && b == 0) || (a == 0 && v == 0) {
                                fm.kind |= flags::SUBSETTED;
                            }
                        }
                        fm.ff_name = Some(buf);
                        if r >= s.len() {
                            break;
                        }
                        u = 0;
                        v = 0;
                    } else {
                        u = a;
                        v = b;
                    }
                }
            }
        }
    }
    // done:
    if fm.ps_name.as_deref().is_some_and(is_std_t1font_name) {
        fm.kind |= flags::STD_T1FONT;
    }
    match &fm.ff_name {
        Some(ff) if ff.len() > 3 => {
            let ext = &ff[ff.len().saturating_sub(4)..];
            if ext.eq_ignore_ascii_case(b".ttf") || ext.eq_ignore_ascii_case(b".ttc") {
                fm.kind |= flags::TRUETYPE;
            } else if ext.eq_ignore_ascii_case(b".otf") {
                fm.kind |= flags::OTF;
            } else {
                fm.kind |= flags::TYPE1;
            }
        }
        _ if fm.ps_name.is_none() => fm.kind |= flags::PK,
        _ => fm.kind |= flags::TYPE1,
    }
    check(&mut fm).then_some(fm)
}

/// `check_fm_entry` (`mapfile.c`) without its warnings: whether the entry
/// is valid (a font file not included is dropped, as pdfTeX does).
fn check(fm: &mut MapEntry) -> bool {
    if fm.is_fontfile() && !fm.is_included() {
        fm.ff_name = None;
    }
    let t1_included = fm.is_fontfile() && fm.is_type1() && fm.is_included();
    !(fm.tfm_name.is_empty()
        || (fm.is_truetype() && fm.is_reencoded() && !fm.is_subsetted())
        || ((fm.slant != 0 || fm.extend != 0) && (fm.tfm_name.is_empty() || !t1_included))
        || fm.slant.abs() > 1000
        || fm.extend.abs() > 2000
        || (fm.is_fontfile() && fm.is_pk())
        || (fm.ps_name.is_some() && fm.is_pk()))
}

/// A map file read as `fm_read_info` reads it: the lines as
/// `append_char_to_buf` keeps them, entries by TFM name, the first entry
/// of a name kept (`FM_DUPIGNORE`).
#[must_use]
pub fn parse_map(bytes: &[u8]) -> BTreeMap<Vec<u8>, MapEntry> {
    let mut map = BTreeMap::new();
    for fm in map_lines(bytes) {
        map.entry(fm.tfm_name.clone()).or_insert(fm);
    }
    map
}

/// A map file's `ps_tree`: the entries by PostScript name, slant and
/// extend, of the lines kept by TFM name that are Type 1 fonts with a
/// font file included, the first of a key kept.
#[must_use]
pub fn parse_map_ps(bytes: &[u8]) -> BTreeMap<(Vec<u8>, i32, i32), MapEntry> {
    let mut tfm = std::collections::BTreeSet::new();
    let mut ps = BTreeMap::new();
    for fm in map_lines(bytes) {
        if !tfm.insert(fm.tfm_name.clone()) {
            continue;
        }
        if let Some(n) = fm.ps_name.clone()
            && fm.is_type1()
            && fm.is_fontfile()
            && fm.is_included()
        {
            ps.entry((n, fm.slant, fm.extend)).or_insert(fm);
        }
    }
    ps
}

/// The entries of a map file's lines, in order.
fn map_lines(bytes: &[u8]) -> Vec<MapEntry> {
    let mut map = Vec::new();
    let mut pos = 0;
    while pos < bytes.len() {
        let mut line = Vec::new();
        loop {
            let c = bytes.get(pos).copied();
            pos += 1;
            let c = match c {
                Some(9) => b' ',
                Some(13) | None => 10,
                Some(c) => c,
            };
            if c != b' ' || line.last().is_some_and(|&l| l != b' ') {
                line.push(c);
            }
            if c == 10 {
                break;
            }
        }
        line.pop();
        if let Some(fm) = parse_line(&line) {
            map.push(fm);
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines() {
        let fm = parse_line(b"cmr10 CMR10 <cmr10.pfb").unwrap();
        assert_eq!(fm.ps_name.as_deref(), Some(&b"CMR10"[..]));
        assert_eq!(fm.ff_name.as_deref(), Some(&b"cmr10.pfb"[..]));
        assert!(fm.is_included() && fm.is_subsetted() && fm.is_type1());
        let fm = parse_line(
            b"ec-lmr10 LMRoman10-Regular \"enclmec ReEncodeFont\" <lm-ec.enc <lmr10.pfb",
        )
        .unwrap();
        assert_eq!(fm.encname.as_deref(), Some(&b"lm-ec.enc"[..]));
        let fm = parse_line(b"pagk8r URWGothicL-Book \".167 SlantFont TeXBase1Encoding ReEncodeFont\" <8r.enc <<uagk8a.pfb").unwrap();
        assert_eq!(fm.slant, 167);
        assert!(fm.is_included() && !fm.is_subsetted());
        assert!(parse_line(b"% comment").is_none());
    }
}
