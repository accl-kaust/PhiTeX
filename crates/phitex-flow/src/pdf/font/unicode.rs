//! ToUnicode maps (pdfTeX's tounicode.c): what `\pdfglyphtounicode`
//! defines (`def`), and the CMap the fold writes for a font from the
//! glyph names of its encoding (`cmap`), reading the definitions that
//! reach the end of the job. Pure functions.

use std::fmt::Write as _;
use std::sync::Arc;

use super::enc::NOTDEF;

/// A glyph name's definition (`glyph_unicode_entry`): a code (-1:
/// `UNI_UNDEF`, an entry all the same) or a string of hexadecimal digits
/// (`UNI_STRING`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Uni {
    Code(i64),
    Seq(Arc<str>),
}

/// `deftounicode`: the definition `\pdfglyphtounicode{glyph}{unistr}`
/// makes, or the warning pdfTeX gives instead (and defines nothing); a
/// warning with a definition when the value is out of range.
pub fn def(glyph: &[u8], unistr: &[u8]) -> (Option<(Vec<u8>, Uni)>, Option<String>) {
    let mut p = unistr;
    while p.first() == Some(&b' ') {
        p = &p[1..];
    }
    let mut l = p.len();
    while l > 0 && p[l - 1] == b' ' {
        l -= 1;
    }
    let mut valid = 1;
    for &c in &p[..l] {
        if c == b' ' {
            valid = 2;
        } else if !is_xdigit(c) {
            valid = 0;
            break;
        }
    }
    if l == 0 || valid == 0 || glyph.is_empty() || glyph == NOTDEF {
        let w = format!(
            "ToUnicode: invalid parameter(s): `{}' => `{}'",
            String::from_utf8_lossy(glyph),
            String::from_utf8_lossy(p)
        );
        return (None, Some(w));
    }
    if valid == 2 {
        let s: String = p.iter().filter(|&&c| c != b' ').map(|&c| char::from(c)).collect();
        return (Some((glyph.to_vec(), Uni::Seq(Arc::from(s)))), None);
    }
    // (`sscanf("%lX")`: an unsigned long, saturating)
    let mut v: u64 = 0;
    for &c in p.iter().take_while(|c| c.is_ascii_hexdigit()) {
        v = v.saturating_mul(16).saturating_add(u64::from(hex(c)));
    }
    if v > 0x10_FFFF {
        let w = format!("ToUnicode: value out of range [0,10FFFF]: {v:X}");
        return (Some((glyph.to_vec(), Uni::Code(-1))), Some(w));
    }
    (Some((glyph.to_vec(), Uni::Code(v as i64))), None)
}

/// tounicode.c's `isXdigit`: digits and upper-case `A`–`F` only.
fn is_xdigit(c: u8) -> bool {
    c.is_ascii_digit() || (b'A'..=b'F').contains(&c)
}

fn hex(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        _ => c - b'A' + 10,
    }
}

/// What a glyph name maps to (`gtab[i]`): nothing (`UNI_UNDEF`), a code,
/// or a string (`UNI_STRING`, `UNI_EXTRA_STRING`).
#[derive(Clone, PartialEq, Eq, Debug)]
enum G {
    Undef,
    Code(i64),
    Str(String),
}

/// `check_unicode_value`.
fn check_unicode_value(s: &[u8], multiple: bool) -> Option<i64> {
    let l = s.len();
    if l == 0 || (multiple && !l.is_multiple_of(4)) || (!multiple && !(4..=6).contains(&l)) {
        return None;
    }
    let mut code = 0i64;
    for i in 0..l {
        if !is_xdigit(s[i]) {
            return None;
        }
        if multiple {
            if i % 4 == 3 {
                code = s[i - 3..=i].iter().fold(0, |a, &c| a * 16 + i64::from(hex(c)));
                if !((0..=0xD7FF).contains(&code) || (0xE000..=0xFFFF).contains(&code)) {
                    return None;
                }
            }
        } else if i == l - 1 {
            code = s.iter().fold(0, |a, &c| a * 16 + i64::from(hex(c)));
            if !((0..=0xD7FF).contains(&code) || (0xE000..=0x10_FFFF).contains(&code)) {
                return None;
            }
        }
    }
    Some(code)
}

/// `utf16be_str`.
fn utf16be(code: i64) -> String {
    if code <= 0xFFFF {
        format!("{code:04X}")
    } else {
        let v = code - 0x10000;
        format!("{:04X}{:04X}", v / 0x400 + 0xD800, v % 0x400 + 0xDC00)
    }
}

/// `set_glyph_unicode`: glyph `s` of font `tfm`, the definitions read by
/// `lookup`.
fn glyph(s: &[u8], tfm: &str, lookup: &mut dyn FnMut(&str) -> Option<Uni>) -> G {
    if s == NOTDEF {
        return G::Undef;
    }
    // (everything after the first dot goes)
    let s = match s.iter().position(|&c| c == b'.') {
        Some(i) => &s[..i],
        None => s,
    };
    if s.is_empty() {
        return G::Undef;
    }
    if s.contains(&b'_') {
        let mut out = String::new();
        for part in s.split(|&c| c == b'_') {
            match glyph(part, tfm, lookup) {
                G::Undef => {}
                G::Str(x) => out.push_str(&x),
                G::Code(c) => out.push_str(&utf16be(c)),
            }
        }
        return G::Str(out);
    }
    let name: String = s.iter().map(|&c| char::from(c)).collect();
    let found = |u: Uni| match u {
        Uni::Code(c) if c < 0 => G::Undef,
        Uni::Code(c) => G::Code(c),
        Uni::Seq(x) => G::Str(x.to_string()),
    };
    if let Some(u) = lookup(&format!("tfm:{tfm}/{name}")) {
        return found(u);
    }
    if let Some(u) = lookup(&name) {
        return found(u);
    }
    if let Some(p) = s.strip_prefix(b"uni") {
        return match check_unicode_value(p, true) {
            Some(c) if p.len() == 4 => G::Code(c),
            Some(_) => G::Str(p.iter().map(|&c| char::from(c)).collect()),
            None => G::Undef,
        };
    }
    if let Some(p) = s.strip_prefix(b"u")
        && let Some(c) = check_unicode_value(p, false)
    {
        return G::Code(c);
    }
    G::Undef
}

/// `is_last_byte_valid`.
fn last_byte_valid(src1: usize, src2: usize, code: i64) -> bool {
    let s = utf16be(code);
    let l = i64::from_str_radix(&s[s.len() - 2..], 16).unwrap_or(0);
    l < 255 - (src2 as i64 - src1 as i64)
}

/// `write_tounicode`'s CMap name (`TFM-ENC`, `TFM-builtin`), and the
/// warning a dubious encoding file name gives.
pub fn cmap_name(tfm: &str, enc: Option<&str>) -> (String, Option<String>) {
    let mut buf = format!("{tfm}-");
    let Some(e) = enc else {
        buf.push_str("builtin");
        return (buf, None);
    };
    buf.push_str(e);
    match buf.rfind('.') {
        Some(i) if &buf[i..] == ".enc" => {
            buf.truncate(i);
            (buf, None)
        }
        _ => (buf, Some(format!("Dubious encoding file name: `{e}'"))),
    }
}

/// `write_tounicode`'s stream (before compression): the CMap of a font
/// whose 256 codes are the glyphs `names`.
pub fn cmap(names: &[Vec<u8>], tfm: &str, buf: &str, lookup: &mut dyn FnMut(&str) -> Option<Uni>) -> Vec<u8> {
    let mut o = format!(
        "%!PS-Adobe-3.0 Resource-CMap\n\
         %%DocumentNeededResources: ProcSet (CIDInit)\n\
         %%IncludeResource: ProcSet (CIDInit)\n\
         %%BeginResource: CMap (TeX-{buf}-0)\n\
         %%Title: (TeX-{buf}-0 TeX {buf} 0)\n\
         %%Version: 1.000\n\
         %%EndComments\n\
         /CIDInit /ProcSet findresource begin\n\
         12 dict begin\n\
         begincmap\n\
         /CIDSystemInfo\n\
         << /Registry (TeX)\n\
         /Ordering ({buf})\n\
         /Supplement 0\n\
         >> def\n\
         /CMapName /TeX-{buf}-0 def\n\
         /CMapType 2 def\n\
         1 begincodespacerange\n\
         <00> <FF>\n\
         endcodespacerange\n"
    );
    let mut gtab: Vec<G> = (0..256).map(|i| names.get(i).map_or(G::Undef, |n| glyph(n, tfm, lookup))).collect();
    gtab.push(G::Undef);
    let code = |g: &G| if let G::Code(c) = g { *c } else { -1 };
    let mut range = [0usize; 257];
    let mut i = 0;
    while i < 256 {
        match &gtab[i] {
            G::Str(_) => {
                range[i] = 1;
                i += 1;
            }
            G::Undef => {
                range[i] = 0;
                i += 1;
            }
            G::Code(_) => {
                let j = i;
                while i < 256
                    && code(&gtab[i + 1]) >= 0
                    && code(&gtab[i]) + 1 == code(&gtab[i + 1])
                    && last_byte_valid(j, i, code(&gtab[i]))
                {
                    i += 1;
                }
                i += 1;
                range[j] = i - j;
            }
        }
    }
    let (mut bfrange, mut bfchar) = (0, 0);
    let mut i = 0;
    while i < 256 {
        match range[i] {
            1 => {
                bfchar += 1;
                i += 1;
            }
            0 => i += 1,
            r => {
                bfrange += 1;
                i += r;
            }
        }
    }
    let mut i = 0;
    loop {
        let sub = bfrange.min(100);
        bfrange -= sub;
        let _ = writeln!(o, "{sub} beginbfrange");
        for _ in 0..sub {
            while i < 256 && range[i] <= 1 {
                i += 1;
            }
            let _ = writeln!(o, "<{i:02X}> <{:02X}> <{}>", i + range[i] - 1, utf16be(code(&gtab[i])));
            i += range[i];
        }
        o.push_str("endbfrange\n");
        if bfrange == 0 {
            break;
        }
    }
    let mut i = 0;
    loop {
        let sub = bfchar.min(100);
        bfchar -= sub;
        let _ = writeln!(o, "{sub} beginbfchar");
        for _ in 0..sub {
            while i < 256 {
                if range[i] > 1 {
                    i += range[i];
                } else if range[i] == 0 {
                    i += 1;
                } else {
                    break;
                }
            }
            match &gtab[i] {
                G::Str(s) => {
                    let _ = writeln!(o, "<{i:02X}> <{s}>");
                }
                g => {
                    let _ = writeln!(o, "<{i:02X}> <{}>", utf16be(code(g)));
                }
            }
            i += 1;
        }
        o.push_str("endbfchar\n");
        if bfchar == 0 {
            break;
        }
    }
    o.push_str(
        "endcmap\n\
         CMapName currentdict /CMap defineresource pop\n\
         end\n\
         end\n\
         %%EndResource\n\
         %%EOF\n",
    );
    o.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        let mut look = |n: &str| match n {
            "A" => Some(Uni::Code(0x41)),
            "B" => Some(Uni::Code(0x42)),
            "ff" => Some(Uni::Seq(Arc::from("00660066"))),
            "tfm:x/A" => Some(Uni::Code(0x61)),
            _ => None,
        };
        assert_eq!(glyph(b"A", "cmr10", &mut look), G::Code(0x41));
        assert_eq!(glyph(b"A", "x", &mut look), G::Code(0x61));
        assert_eq!(glyph(b"A.sc", "cmr10", &mut look), G::Code(0x41));
        assert_eq!(glyph(b"A_B", "cmr10", &mut look), G::Str("00410042".into()));
        assert_eq!(glyph(b"uni00410042", "cmr10", &mut look), G::Str("00410042".into()));
        assert_eq!(glyph(b"uni0041", "cmr10", &mut look), G::Code(0x41));
        assert_eq!(glyph(b"u1D400", "cmr10", &mut look), G::Code(0x1D400));
        assert_eq!(utf16be(0x1D400), "D835DC00");
        assert_eq!(def(b"A", b" 0041 "), (Some((b"A".to_vec(), Uni::Code(0x41))), None));
        assert_eq!(def(b"ff", b"0066 0066").0, Some((b"ff".to_vec(), Uni::Seq(Arc::from("00660066")))));
        assert!(def(b"A", b"00x1").0.is_none());
    }
}
