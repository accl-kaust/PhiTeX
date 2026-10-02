//! tounicode.c: `/ToUnicode` `CMaps` from glyph names (`\pdfgentounicode`),
//! resolved through `\pdfglyphtounicode` entries and the `uniXXXX` and
//! `uXXXX[XX]` naming conventions.

use alloc::format;
use alloc::vec::Vec;

use super::enc::NOTDEF;
use crate::host::Host;
use crate::pdfconv::ToUnicode;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;

/// A glyph's Unicode: a code point, a UTF-16BE hex string
/// (`UNI_STRING`, `UNI_EXTRA_STRING`), or none (`UNI_UNDEF`).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Uni {
    Undef,
    Code(u32),
    Str(Vec<u8>),
}

fn is_xdigit(c: u8) -> bool {
    c.is_ascii_digit() || (b'A'..=b'F').contains(&c)
}

fn hex(s: &[u8]) -> u32 {
    s.iter().fold(0u32, |v, &c| {
        v.wrapping_mul(16)
            .wrapping_add(char::from(c).to_digit(16).unwrap_or(0))
    })
}

/// `check_unicode_value`.
fn check_unicode_value(s: &[u8], multiple: bool) -> Option<u32> {
    let l = s.len();
    if l == 0 || (multiple && !l.is_multiple_of(4)) || (!multiple && !(4..=6).contains(&l)) {
        return None;
    }
    if !s.iter().all(|&c| is_xdigit(c)) {
        return None;
    }
    if multiple {
        let mut code = 0;
        for g in s.chunks(4) {
            code = hex(g);
            if !((0..=0xD7FF).contains(&code) || (0xE000..=0xFFFF).contains(&code)) {
                return None;
            }
        }
        Some(code)
    } else {
        let code = hex(s);
        ((0..=0xD7FF).contains(&code) || (0xE000..=0x10_FFFF).contains(&code)).then_some(code)
    }
}

/// `utf16be_str`.
fn utf16be(code: u32) -> Vec<u8> {
    if code <= 0xFFFF {
        format!("{code:04X}").into_bytes()
    } else {
        let v = code - 0x10000;
        format!("{:04X}{:04X}", v / 0x400 + 0xD800, v % 0x400 + 0xDC00).into_bytes()
    }
}

fn from_entry(e: &ToUnicode) -> Uni {
    match e {
        ToUnicode::Code(c) => Uni::Code(*c),
        ToUnicode::Text(t) => Uni::Str(t.clone()),
        ToUnicode::Undefined => Uni::Undef,
    }
}

/// `set_glyph_unicode`.
fn glyph_unicode(s: &[u8], tfm: &[u8], tree: &crate::pdfconv::ToUnicodeTable) -> Uni {
    if s == NOTDEF {
        return Uni::Undef;
    }
    let s = s.iter().position(|&c| c == b'.').map_or(s, |i| &s[..i]);
    if s.is_empty() {
        return Uni::Undef;
    }
    if s.contains(&b'_') {
        let mut out = Vec::new();
        for part in s.split(|&c| c == b'_') {
            match glyph_unicode(part, tfm, tree) {
                Uni::Undef => {}
                Uni::Str(t) => out.extend_from_slice(&t),
                Uni::Code(c) => out.extend_from_slice(&utf16be(c)),
            }
        }
        return Uni::Str(out);
    }
    let mut key = b"tfm:".to_vec();
    key.extend_from_slice(tfm);
    key.push(b'/');
    key.extend_from_slice(s);
    if let Some(e) = tree.get(&key).or_else(|| tree.get(&s.to_vec())) {
        return from_entry(e);
    }
    if let Some(p) = s.strip_prefix(b"uni") {
        return match check_unicode_value(p, true) {
            Some(c) if p.len() == 4 => Uni::Code(c),
            Some(_) => Uni::Str(p.to_vec()),
            None => Uni::Undef,
        };
    }
    if let Some(p) = s.strip_prefix(b"u")
        && let Some(c) = check_unicode_value(p, false)
    {
        return Uni::Code(c);
    }
    Uni::Undef
}

/// The `CMap`'s name: `tfm-enc` (`.enc` cut) or `tfm-builtin`, and the
/// dubious-name warning.
fn cmap_name(tfm: &[u8], enc: Option<&[u8]>) -> (Vec<u8>, Option<Vec<u8>>) {
    let mut buf = tfm.to_vec();
    buf.push(b'-');
    let mut warn = None;
    if let Some(e) = enc {
        let start = buf.len();
        buf.extend_from_slice(e);
        match buf[start..].iter().rposition(|&c| c == b'.') {
            Some(d) if &buf[start + d..] == b".enc" => buf.truncate(start + d),
            _ => {
                let mut m = b"Dubious encoding file name: `".to_vec();
                m.extend_from_slice(e);
                m.push(b'\'');
                warn = Some(m);
            }
        }
    } else {
        buf.extend_from_slice(b"builtin");
    }
    (buf, warn)
}

/// The `CMap` stream's text for `names` (`write_tounicode` from the
/// header on).
fn cmap(
    names: &[Vec<u8>],
    tfm: &[u8],
    name: &[u8],
    tree: &crate::pdfconv::ToUnicodeTable,
) -> Vec<u8> {
    let b = alloc::string::String::from_utf8_lossy(name);
    let mut o = format!(
        "%!PS-Adobe-3.0 Resource-CMap\n\
         %%DocumentNeededResources: ProcSet (CIDInit)\n\
         %%IncludeResource: ProcSet (CIDInit)\n\
         %%BeginResource: CMap (TeX-{b}-0)\n\
         %%Title: (TeX-{b}-0 TeX {b} 0)\n\
         %%Version: 1.000\n\
         %%EndComments\n\
         /CIDInit /ProcSet findresource begin\n\
         12 dict begin\n\
         begincmap\n\
         /CIDSystemInfo\n\
         << /Registry (TeX)\n\
         /Ordering ({b})\n\
         /Supplement 0\n\
         >> def\n\
         /CMapName /TeX-{b}-0 def\n\
         /CMapType 2 def\n\
         1 begincodespacerange\n\
         <00> <FF>\n\
         endcodespacerange\n"
    )
    .into_bytes();
    let mut g: Vec<Uni> = (0..256)
        .map(|i| {
            names
                .get(i)
                .map_or(Uni::Undef, |n| glyph_unicode(n, tfm, tree))
        })
        .collect();
    g.push(Uni::Undef);
    let code = |u: &Uni| match u {
        Uni::Code(c) => Some(*c),
        _ => None,
    };
    let mut range = [0usize; 257];
    let mut i = 0;
    while i < 256 {
        match &g[i] {
            Uni::Str(_) => {
                range[i] = 1;
                i += 1;
            }
            Uni::Undef => i += 1,
            Uni::Code(_) => {
                let j = i;
                while i < 256
                    && let (Some(a), Some(b)) = (code(&g[i]), code(&g[i + 1]))
                    && a + 1 == b
                    && {
                        // `is_last_byte_valid`
                        let s = utf16be(a);
                        let l = hex(&s[s.len() - 2..]);
                        i64::from(l) < 255 - i64::try_from(i - j).unwrap_or(0)
                    }
                {
                    i += 1;
                }
                i += 1;
                range[j] = i - j;
            }
        }
    }
    let (mut nrange, mut nchar) = (0, 0);
    let mut i = 0;
    while i < 256 {
        match range[i] {
            1 => {
                nchar += 1;
                i += 1;
            }
            0 => i += 1,
            r => {
                nrange += 1;
                i += r;
            }
        }
    }
    let mut i = 0;
    loop {
        let sub = nrange.min(100);
        nrange -= sub;
        o.extend_from_slice(format!("{sub} beginbfrange\n").as_bytes());
        for _ in 0..sub {
            while i < 256 && range[i] <= 1 {
                i += 1;
            }
            let c = code(&g[i]).unwrap_or(0);
            o.extend_from_slice(format!("<{i:02X}> <{:02X}> <", i + range[i] - 1).as_bytes());
            o.extend_from_slice(&utf16be(c));
            o.extend_from_slice(b">\n");
            i += range[i];
        }
        o.extend_from_slice(b"endbfrange\n");
        if nrange == 0 {
            break;
        }
    }
    let mut i = 0;
    loop {
        let sub = nchar.min(100);
        nchar -= sub;
        o.extend_from_slice(format!("{sub} beginbfchar\n").as_bytes());
        for _ in 0..sub {
            while i < 256 && range[i] != 1 {
                i += range[i].max(1);
            }
            o.extend_from_slice(format!("<{i:02X}> <").as_bytes());
            match &g[i] {
                Uni::Str(s) => o.extend_from_slice(s),
                Uni::Code(c) => o.extend_from_slice(&utf16be(*c)),
                Uni::Undef => {}
            }
            o.extend_from_slice(b">\n");
            i += 1;
        }
        o.extend_from_slice(b"endbfchar\n");
        if nchar == 0 {
            break;
        }
    }
    o.extend_from_slice(
        b"endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n%%EndResource\n%%EOF\n",
    );
    o
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// `write_tounicode`: the `CMap` object, or 0 (with `\pdfgentounicode`
    /// then off) if no `\pdfglyphtounicode` was given.
    pub(crate) fn write_tounicode(
        &mut self,
        names: &[Vec<u8>],
        tfm: &[u8],
        enc: Option<&[u8]>,
    ) -> Result<i32, Jump> {
        if self.tounicode.is_empty() {
            self.pdftex_warn(b"no GlyphToUnicode entry has been inserted yet!");
            self.pdf.fontw.gen_tounicode = 0;
            return Ok(0);
        }
        let (name, warn) = cmap_name(tfm, enc);
        if let Some(w) = warn {
            self.pdftex_warn(&w);
        }
        let text = cmap(names, tfm, &name, &self.tounicode);
        let objnum = self.pdf_new_objnum()?;
        self.pdf_begin_dict(objnum, 0)?;
        self.pdf_begin_stream();
        self.pdf.out.print(&text);
        self.pdf_end_stream();
        Ok(objnum)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_glyph_names() {
        let mut t = crate::pdfconv::ToUnicodeTable::default();
        t.insert(b"A".to_vec(), ToUnicode::Code(0x41));
        t.insert(b"ff".to_vec(), ToUnicode::Text(b"00660066".to_vec()));
        t.insert(b"tfm:cmr10/A".to_vec(), ToUnicode::Code(0x391));
        assert_eq!(glyph_unicode(b"A", b"cmr10", &t), Uni::Code(0x391));
        assert_eq!(glyph_unicode(b"A.sc", b"x", &t), Uni::Code(0x41));
        assert_eq!(
            glyph_unicode(b"uni00410042", b"x", &t),
            Uni::Str(b"00410042".to_vec())
        );
        assert_eq!(glyph_unicode(b"u1D400", b"x", &t), Uni::Code(0x1D400));
        assert_eq!(glyph_unicode(b"union", b"x", &t), Uni::Undef);
        assert_eq!(
            glyph_unicode(b"A_ff", b"x", &t),
            Uni::Str(b"004100660066".to_vec())
        );
        assert_eq!(utf16be(0x1D400), b"D835DC00");
    }

    #[test]
    fn writes_ranges_and_chars() {
        let mut t = crate::pdfconv::ToUnicodeTable::default();
        for (n, c) in [("A", 0x41), ("B", 0x42), ("C", 0x43)] {
            t.insert(n.as_bytes().to_vec(), ToUnicode::Code(c));
        }
        t.insert(b"fi".to_vec(), ToUnicode::Text(b"00660069".to_vec()));
        let mut names = alloc::vec![NOTDEF.to_vec(); 256];
        names[65] = b"A".to_vec();
        names[66] = b"B".to_vec();
        names[67] = b"C".to_vec();
        names[12] = b"fi".to_vec();
        let o = cmap(&names, b"x", b"x-y", &t);
        let s = core::str::from_utf8(&o).unwrap();
        assert!(s.contains("1 beginbfrange\n<41> <43> <0041>\nendbfrange\n"));
        assert!(s.contains("1 beginbfchar\n<0C> <00660069>\nendbfchar\n"));
    }
}
