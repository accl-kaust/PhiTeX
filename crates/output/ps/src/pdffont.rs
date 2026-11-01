//! Font resources as pdfwrite writes them (gdevpdtf.c, gdevpdtw.c,
//! gdevpdtd.c): a resource per font, its Encoding against a base
//! encoding, its widths, the base 14 left unembedded unless
//! `/prepress`, others embedded as a Type1C subset (`XXXXXX+Name`, the
//! prefix hashed from the font file and the codes used), the
//! FontDescriptor's metrics computed from the subset's glyphs, the
//! ToUnicode CMap.

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::font::Font;
use crate::interp::{Ps, R};
use crate::num::pg;
use crate::obj::PDict;
use crate::pdf::Device;

/// The base 14 font names.
const BASE14: [&str; 14] = [
    "Courier",
    "Courier-Bold",
    "Courier-Oblique",
    "Courier-BoldOblique",
    "Helvetica",
    "Helvetica-Bold",
    "Helvetica-Oblique",
    "Helvetica-BoldOblique",
    "Times-Roman",
    "Times-Bold",
    "Times-Italic",
    "Times-BoldItalic",
    "Symbol",
    "ZapfDingbats",
];

/// gs's known encodings by index (gscencs.h).
pub fn known_encoding(index: usize) -> &'static [&'static str] {
    match index {
        0 => &crate::enc::STANDARD_ENCODING,
        1 => &crate::enc::ISO_LATIN1_ENCODING,
        2 => &crate::enc::SYMBOL_ENCODING,
        3 => &crate::enc::DINGBATS_ENCODING,
        4 => &crate::enc::WIN_ANSI_ENCODING,
        5 => &crate::enc::MAC_ROMAN_ENCODING,
        _ => &crate::enc::MAC_EXPERT_ENCODING,
    }
}

const ENCODING_NAMES: [&str; 7] = [
    "StandardEncoding",
    "ISOLatin1Encoding",
    "SymbolEncoding",
    "DingbatsEncoding",
    "WinAnsiEncoding",
    "MacRomanEncoding",
    "MacExpertEncoding",
];

/// A PDF font resource.
pub struct PdfFont {
    /// The object number (0 until `pdf_assign_font_object_id`).
    pub id: i64,
    pub descriptor_id: i64,
    /// The gs font it was made for (its base).
    pub base: u32,
    pub font: alloc::rc::Rc<Font>,
    pub name: Vec<u8>,
    /// A base 14 font left unembedded.
    pub std: bool,
    pub embed: bool,
    pub base_encoding: i32,
    /// The glyph of each code used, and whether it is a difference.
    pub enc: Vec<Option<Vec<u8>>>,
    pub widths: [f64; 256],
    pub real_widths: [f64; 256],
    pub used: [bool; 256],
}

#[derive(Default)]
pub struct Fonts {
    pub list: Vec<PdfFont>,
}

/// `pdf_refine_encoding_index`.
fn refine(index: i32, std: bool) -> i32 {
    match index {
        4..=6 => index,
        0 if std => index,
        _ => 4,
    }
}

/// The resource for the current font and string (`pdf_obtain_font_resource`).
pub fn obtain(ps: &mut Ps<'_>, font: &alloc::rc::Rc<Font>, s: &[u8]) -> R<usize> {
    let d = font.dict.clone();
    let glyphs: Vec<Vec<u8>> = s.iter().map(|&c| ps.glyph_name(&d, c)).collect();
    // (a resource of this font whose encoding agrees)
    let found = ps.dev.fonts.list.iter().position(|f| {
        f.base == font.base
            && s.iter()
                .zip(&glyphs)
                .all(|(&c, g)| f.enc[usize::from(c)].as_ref().is_none_or(|e| e == g))
    });
    let k = match found {
        Some(k) => k,
        None => {
            let name = font.name.clone();
            let is14 = BASE14.iter().any(|b| b.as_bytes() == name.as_slice());
            let std = is14 && !ps.dev.s.prepress && font.resource.is_some();
            let base_encoding = if std && font.nearest_encoding == 0 {
                -1
            } else {
                refine(font.nearest_encoding, std)
            };
            ps.dev.fonts.list.push(PdfFont {
                id: 0,
                descriptor_id: 0,
                base: font.base,
                font: font.clone(),
                name,
                std,
                embed: !std,
                base_encoding,
                enc: alloc::vec![None; 256],
                widths: [0.0; 256],
                real_widths: [0.0; 256],
                used: [false; 256],
            });
            ps.dev.fonts.list.len() - 1
        }
    };
    for (&c, g) in s.iter().zip(glyphs) {
        let f = &mut ps.dev.fonts.list[k];
        if f.enc[usize::from(c)].is_none() {
            f.enc[usize::from(c)] = Some(g);
        }
    }
    Ok(k)
}

/// A character's widths in PDF text space ×1000 (`pdf_char_widths`):
/// (real, Width).
pub fn char_width(ps: &mut Ps<'_>, k: usize, font: &Font, c: u8) -> (f64, f64) {
    let ci = usize::from(c);
    if ps.dev.fonts.list[k].real_widths[ci] == 0.0 {
        let d = font.dict.clone();
        let g = ps.dev.fonts.list[k].enc[ci]
            .clone()
            .unwrap_or_else(|| b".notdef".to_vec());
        let w = ps.glyph_width(&d, &g);
        // (scale_o: the original matrix ×1000, in float)
        let o = font.orig_matrix;
        let s = crate::matrix::M {
            xx: (f64::from(o.xx) * 1000.0) as f32,
            xy: (f64::from(o.xy) * 1000.0) as f32,
            yx: (f64::from(o.yx) * 1000.0) as f32,
            yy: (f64::from(o.yy) * 1000.0) as f32,
            tx: 0.0,
            ty: 0.0,
        };
        let (x, _) = s.dtransform(f64::from(w.0 as f32), f64::from(w.1 as f32));
        let f = &mut ps.dev.fonts.list[k];
        f.real_widths[ci] = x;
        f.widths[ci] = x;
    }
    let f = &ps.dev.fonts.list[k];
    (f.real_widths[ci], f.widths[ci])
}

pub fn mark_used(d: &mut Device, k: usize, s: &[u8]) {
    for &c in s {
        d.fonts.list[k].used[usize::from(c)] = true;
    }
}

/// `pdf_assign_font_object_id`: the font's number (and its
/// FontDescriptor's) at first use.
pub fn assign_id(d: &mut Device, k: usize) -> i64 {
    if d.fonts.list[k].id == 0 {
        let id = d.alloc();
        d.fonts.list[k].id = id;
        if !d.fonts.list[k].std {
            let fd = d.alloc();
            d.fonts.list[k].descriptor_id = fd;
        }
    }
    d.fonts.list[k].id
}

/// The FontDescriptor's values (`pdf_compute_font_descriptor`) over the
/// glyphs embedded.
struct Desc {
    bbox: [i32; 4],
    flags: i32,
    ascent: i32,
    cap_height: i32,
    descent: i32,
    italic_angle: i32,
    stem_v: i32,
    missing_width: i32,
    x_height: i32,
    fixed: Option<i32>,
}

fn compute_descriptor(ps: &mut Ps<'_>, d: &PDict, glyphs: &[Vec<u8>]) -> Desc {
    let mut bbox = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
    let mut ascent = 0;
    let mut missing = 0;
    let mut fixed_width: i32 = 0;
    let mut symbolic = false;
    let (mut x_height, mut cap_height, mut small_height, mut small_descent) =
        (0i32, 0i32, 0i32, 0i32);
    let mut small_present = false;
    let (mut colon, mut period, mut big_i): (Option<[f64; 4]>, Option<[f64; 4]>, Option<[f64; 4]>) =
        (None, None, None);
    let mut notdef_seen = false;
    for g in glyphs {
        let Some(gl) = ps.type1_glyph(d, g) else {
            continue;
        };
        let b = gl.bbox().unwrap_or([0.0; 4]);
        if (b[0] as i32) < bbox[0] || b[0] < f64::from(bbox[0]) {
            bbox[0] = bbox[0].min(b[0] as i32);
        }
        if b[2] > f64::from(bbox[2]) {
            bbox[2] = b[2] as i32;
        }
        if b[1] < f64::from(bbox[1]) {
            bbox[1] = b[1] as i32;
        }
        if b[3] > f64::from(bbox[3]) {
            bbox[3] = b[3] as i32;
        }
        if gl.seac.is_none() {
            ascent = ascent.max(b[3] as i32);
        }
        if !notdef_seen && g.as_slice() == b".notdef" {
            notdef_seen = true;
            missing = gl.width.0 as i32;
        }
        if gl.width.1 != 0.0 {
            fixed_width = i32::MIN;
        } else if fixed_width == 0 {
            fixed_width = gl.width.0 as i32;
        } else if gl.width.0 as i32 != fixed_width {
            fixed_width = i32::MIN;
        }
        if symbolic {
            continue;
        }
        if crate::uni::lookup(g).is_none() && g.as_slice() != b".notdef" && !in_known_encodings(g) {
            symbolic = true;
            continue;
        }
        if g.as_slice() != b".notdef"
            && !crate::enc::STANDARD_ENCODING
                .iter()
                .chain(crate::enc::ISO_LATIN1_ENCODING.iter())
                .chain(crate::enc::WIN_ANSI_ENCODING.iter())
                .chain(crate::enc::MAC_ROMAN_ENCODING.iter())
                .chain(crate::enc::SYMBOL_ENCODING.iter())
                .chain(crate::enc::DINGBATS_ENCODING.iter())
                .chain(crate::enc::MAC_EXPERT_ENCODING.iter())
                .any(|n| n.as_bytes() == g.as_slice())
        {
            symbolic = true;
            continue;
        }
        match g.as_slice() {
            b"colon" => {
                colon = Some(b);
                continue;
            }
            b"period" => {
                period = Some(b);
                continue;
            }
            [c] => {
                let c = *c;
                if c.is_ascii_uppercase() {
                    cap_height = cap_height.max(b[3] as i32);
                    if c == b'I' {
                        big_i = Some(b);
                    }
                } else if c.is_ascii_lowercase() {
                    let (y0, y1) = (b[1] as i32, b[3] as i32);
                    small_present = true;
                    match c {
                        b'b' | b'd' | b'f' | b'h' | b'k' | b'l' | b't' => {
                            small_height = small_height.max(y1)
                        }
                        b'i' => {}
                        b'j' => small_descent = small_descent.min(y0),
                        b'g' | b'p' | b'q' | b'y' => {
                            small_descent = small_descent.min(y0);
                            x_height = x_height.max(y1);
                        }
                        _ => x_height = x_height.max(y1),
                    }
                }
            }
            _ => {}
        }
    }
    let mut flags = 0;
    let mut desc_x = 0;
    let mut desc_cap = 0;
    let mut italic = 0;
    let mut stem_v = 0;
    if symbolic {
        flags |= 4;
    } else {
        flags |= 32;
        desc_x = x_height;
        if !small_present {
            flags |= 1 << 16;
        }
        desc_cap = cap_height;
        if let (Some(c), Some(p)) = (colon, period) {
            let a = libm::atan2((c[3] - c[1]) - (p[3] - p[1]), (c[2] - c[0]) - (p[2] - p[0]))
                * (180.0 / core::f64::consts::PI);
            let mut angle = a as i32 - 90;
            while angle > 90 {
                angle -= 180;
            }
            while angle < -90 {
                angle += 180;
            }
            angle = angle.clamp(-30, 30);
            if (-2..=2).contains(&angle) {
                angle = 0;
            }
            italic = angle;
        }
        if italic != 0 {
            flags |= 64;
        }
        if let Some(bi) = big_i {
            let p = period.unwrap_or([0.0; 4]);
            let wdot = p[2] - p[0];
            let wcolon = bi[2] - bi[0];
            let wi = p[2] - p[0];
            stem_v = wdot as i32;
            if wi > wcolon * 2.5 || wi > (p[3] - p[1]) * 0.25 {
                flags |= 2;
            }
        }
    }
    if ascent == 0 {
        ascent = bbox[3];
    }
    let descent = bbox[1];
    if flags & (4 | (1 << 16)) == 0
        && (f64::from(small_descent) > f64::from(descent) / 3.0
            || f64::from(desc_x) > f64::from(small_height) * 0.9)
    {
        flags |= 1 << 17;
    }
    let mut fixed = None;
    if fixed_width > 0 {
        flags |= 1;
        fixed = Some(fixed_width);
        missing = fixed_width;
    }
    if desc_cap == 0 {
        desc_cap = ascent;
    }
    if stem_v == 0 {
        stem_v = (f64::from(bbox[2]) * 0.15) as i32;
    }
    Desc {
        bbox,
        flags,
        ascent,
        cap_height: desc_cap,
        descent,
        italic_angle: italic,
        stem_v,
        missing_width: missing,
        x_height: desc_x,
        fixed,
    }
}

fn in_known_encodings(g: &[u8]) -> bool {
    (0..7).any(|i| known_encoding(i).iter().any(|n| n.as_bytes() == g))
}

/// psdf_round(v, precision, radix).
pub fn psdf_round(v: f64, precision: i32, radix: i32) -> f64 {
    let mut mul = 1.0;
    let mut w = v;
    if w <= 0.0 {
        return w;
    }
    while w < f64::from(precision) {
        w *= f64::from(radix);
        mul *= f64::from(radix);
    }
    f64::from((w + 0.5) as i32) / mul
}

/// The subset prefix (`pdf_add_subset_prefix`): the font file's MD5
/// (read as C `char`s, signed) and the codes used.
fn subset_prefix(md5: &[u8; 16], used: &[bool; 256]) -> Vec<u8> {
    let mut v: u64 = 0;
    let h = |v: u64, w: u16| v.wrapping_mul(3_141_592_653).wrapping_add(u64::from(w));
    for i in (0..8).step_by(2) {
        let lo = i32::from(md5[i] as i8);
        let hi = i32::from(md5[i + 1] as i8);
        v = h(v, (lo | (hi << 8)) as u16);
    }
    let mut bits = [0u8; 32];
    for (c, &u) in used.iter().enumerate() {
        if u {
            bits[c >> 3] |= 0x80 >> (c & 7);
        }
    }
    for i in (0..32).step_by(2) {
        v = h(v, u16::from(bits[i]) | (u16::from(bits[i + 1]) << 8));
    }
    let mut out = Vec::new();
    for _ in 0..6 {
        out.push(b'A' + (v % 26) as u8);
        v /= 26;
    }
    out.push(b'+');
    out
}

/// The ToUnicode CMap stream's text.
fn to_unicode(id: i64, entries: &[(u8, u32)]) -> Vec<u8> {
    let mut s = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapType 2 def\n",
    );
    s.push_str(&format!(
        "/CMapName/R{id} def\n1 begincodespacerange\n<00><ff>\nendcodespacerange\n"
    ));
    for chunk in entries.chunks(100) {
        s.push_str(&format!("{} beginbfrange\n", chunk.len()));
        for &(c, u) in chunk {
            if u > 0xffff {
                let v = u - 0x10000;
                s.push_str(&format!(
                    "<{c:02x}><{c:02x}><{:04x}{:04x}>\n",
                    0xd800 + (v >> 10),
                    0xdc00 + (v & 0x3ff)
                ));
            } else {
                s.push_str(&format!("<{c:02x}><{c:02x}><{u:04x}>\n"));
            }
        }
        s.push_str("endbfrange\n");
    }
    s.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend end\n");
    s.into_bytes()
}

/// Writes the fonts at the document's close.
pub fn write_fonts(ps: &mut Ps<'_>) {
    let n = ps.dev.fonts.list.len();
    for k in 0..n {
        if ps.dev.fonts.list[k].id == 0 {
            continue;
        }
        let (font, embed) = (
            ps.dev.fonts.list[k].font.clone(),
            ps.dev.fonts.list[k].embed,
        );
        let d = font.dict.clone();
        // (the glyphs: .notdef, those used, seac components)
        let mut set: BTreeSet<Vec<u8>> = BTreeSet::new();
        set.insert(b".notdef".to_vec());
        for c in 0..256 {
            if ps.dev.fonts.list[k].used[c]
                && let Some(g) = ps.dev.fonts.list[k].enc[c].clone()
            {
                if let Some(gl) = ps.type1_glyph(&d, &g)
                    && let Some((_, _, _, b, a)) = gl.seac
                {
                    set.insert(
                        crate::enc::STANDARD_ENCODING[usize::from(b)]
                            .as_bytes()
                            .to_vec(),
                    );
                    set.insert(
                        crate::enc::STANDARD_ENCODING[usize::from(a)]
                            .as_bytes()
                            .to_vec(),
                    );
                }
                set.insert(g);
            }
        }
        let glyphs: Vec<Vec<u8>> = set.into_iter().collect();
        // (pdf_finish_FontDescriptor: the font file)
        let mut ff: Option<(i64, Vec<u8>)> = None;
        let mut desc = None;
        if embed {
            let dsc = compute_descriptor(ps, &d, &glyphs);
            let ff_id = ps.dev.alloc();
            let used = ps.dev.fonts.list[k].used;
            let enc = ps.dev.fonts.list[k].enc.clone();
            let data = crate::cff::write(ps, &font, &glyphs, &enc, &used, &dsc.bbox);
            let z = phitex_codecs::deflate::compress(&data, 6);
            ff = Some((ff_id, z));
            desc = Some(dsc);
        }
        let f = &ps.dev.fonts.list[k];
        let mut base_name = f.name.clone();
        if let Some((_, z)) = &ff {
            let md5 = phitex_codecs::md5::md5(z);
            let mut p = subset_prefix(&md5, &f.used);
            p.extend_from_slice(&base_name);
            base_name = p;
        }
        // (pdf_write_font_resource)
        let tu_id = ps.dev.alloc();
        let f = &ps.dev.fonts.list[k];
        let mut entries: Vec<(u8, u32)> = Vec::new();
        for c in 0..256 {
            if let Some(g) = &f.enc[c]
                && f.used[c]
                && let Some(u) = crate::uni::lookup(g)
            {
                entries.push((c as u8, u));
            }
        }
        let cmap = to_unicode(tu_id, &entries);
        let z = phitex_codecs::deflate::compress(&cmap, 6);
        let mut o = format!(
            "{tu_id} 0 obj\n<</Filter/FlateDecode/Length {}>>stream\n",
            z.len()
        )
        .into_bytes();
        o.extend_from_slice(&z);
        o.extend_from_slice(b"\nendstream\nendobj\n");
        ps.dev.direct.push((tu_id, o));
        let f = &ps.dev.fonts.list[k];
        let mut b = String::from("<</BaseFont/");
        b.push_str(&String::from_utf8_lossy(&base_name));
        if f.descriptor_id != 0 {
            b.push_str(&format!("/FontDescriptor {} 0 R", f.descriptor_id));
        }
        b.push_str(&format!("/ToUnicode {tu_id} 0 R/Type/Font\n"));
        if !f.std {
            let first = (0..256).find(|&c| f.used[c]).unwrap_or(0);
            let last = (0..256).rev().find(|&c| f.used[c]).unwrap_or(0);
            b.push_str(&format!("/FirstChar {first}/LastChar {last}/Widths["));
            for c in first..=last {
                let w = if f.used[c] {
                    psdf_round(f.widths[c], 100, 10)
                } else {
                    0.0
                };
                b.push_str(if c & 15 != 0 { " " } else { "\n" });
                b.push_str(&pg(w));
            }
            b.push_str("]\n");
        }
        // (the Encoding: differences against the base)
        let base: Option<&[&str]> = if f.base_encoding >= 0 {
            Some(known_encoding(f.base_encoding as usize))
        } else {
            None
        };
        let differs = |c: usize| match (&f.enc[c], base) {
            (Some(g), Some(be)) => be[c].as_bytes() != g.as_slice(),
            (Some(g), None) => crate::enc::STANDARD_ENCODING[c].as_bytes() != g.as_slice(),
            _ => false,
        };
        let first_diff = (0..256).find(|&c| f.used[c] && differs(c));
        let diff_id = if first_diff.is_some() {
            ps.dev.next_obj
        } else {
            0
        };
        if diff_id != 0 {
            b.push_str(&format!("/Encoding {diff_id} 0 R"));
        } else if f.base_encoding > 0 {
            b.push_str(&format!(
                "/Encoding/{}",
                ENCODING_NAMES[f.base_encoding as usize]
            ));
        }
        b.push_str("/Subtype/Type1>>\n");
        let mut enc_body = None;
        if let Some(c0) = first_diff {
            let mut e = String::from("<</Type/Encoding");
            if f.base_encoding > 0 {
                e.push_str(&format!(
                    "/BaseEncoding/{}",
                    ENCODING_NAMES[f.base_encoding as usize]
                ));
            }
            e.push_str("/Differences[");
            let mut prev = 256usize;
            let mut cnt = 0;
            for c in c0..256 {
                if !(f.used[c] && differs(c)) {
                    continue;
                }
                if c != prev + 1 || prev == 256 {
                    e.push_str(&format!("\n{c}"));
                    cnt = 1;
                } else {
                    if cnt & 15 == 0 {
                        e.push('\n');
                    }
                    cnt += 1;
                }
                e.push('/');
                e.push_str(&String::from_utf8_lossy(
                    f.enc[c].as_deref().unwrap_or(b".notdef"),
                ));
                prev = c;
            }
            e.push_str("]>>\n");
            enc_body = Some(e);
        }
        let font_id = f.id;
        let fd_id = f.descriptor_id;
        ps.dev.write_separate(font_id, b.into_bytes());
        if let Some(e) = enc_body {
            let id = ps.dev.alloc();
            ps.dev.write_separate(id, e.into_bytes());
        }
        // (the FontDescriptor, then the font file)
        if let (Some(dsc), Some((ff_id, z))) = (desc, ff) {
            let mut fd = String::from("<</Type/FontDescriptor/FontName/");
            fd.push_str(&String::from_utf8_lossy(&base_name));
            fd.push_str(&format!(
                "/FontBBox[{} {} {} {}]/Flags {}\n",
                dsc.bbox[0], dsc.bbox[1], dsc.bbox[2], dsc.bbox[3], dsc.flags
            ));
            fd.push_str(&format!(
                "/Ascent {}\n/CapHeight {}\n/Descent {}\n/ItalicAngle {}\n/StemV {}\n",
                dsc.ascent, dsc.cap_height, dsc.descent, dsc.italic_angle, dsc.stem_v
            ));
            if let Some(w) = dsc.fixed {
                fd.push_str(&format!("/AvgWidth {w}\n/MaxWidth {w}\n"));
            }
            if dsc.missing_width != 0 {
                fd.push_str(&format!("/MissingWidth {}\n", dsc.missing_width));
            }
            if dsc.x_height != 0 {
                fd.push_str(&format!("/XHeight {}\n", dsc.x_height));
            }
            fd.push_str("/CharSet(");
            let mut names: Vec<&Vec<u8>> = glyphs
                .iter()
                .filter(|g| g.as_slice() != b".notdef")
                .collect();
            names.sort();
            for g in names {
                fd.push('/');
                fd.push_str(&String::from_utf8_lossy(g));
            }
            fd.push_str(&format!(")/FontFile3 {ff_id} 0 R>>\n"));
            ps.dev.write_separate(fd_id, fd.into_bytes());
            let mut o = format!(
                "{ff_id} 0 obj\n<</Filter/FlateDecode\n/Subtype/Type1C/Length {}>>stream\n",
                z.len()
            )
            .into_bytes();
            o.extend_from_slice(&z);
            o.extend_from_slice(b"\nendstream\nendobj\n");
            ps.dev.direct.push((ff_id, o));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_of_revtex_fig() {
        // (gs 10.08.0 on revtex's fig_1.eps: its FontFile3's MD5, and
        // "Test Figure" shown, give DYYRMW+)
        let md5 = [
            0x61, 0xd9, 0x52, 0xf6, 0x8b, 0x08, 0x06, 0x46, 0x75, 0xe2, 0x50, 0x91, 0xd5, 0x87,
            0x92, 0xaa,
        ];
        let mut used = [false; 256];
        for &c in b"Test Figure" {
            used[usize::from(c)] = true;
        }
        assert_eq!(subset_prefix(&md5, &used), b"DYYRMW+");
    }
}
