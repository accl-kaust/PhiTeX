//! Font replacement in included PDF files (`pdftoepdf.cc`'s `copyFont`,
//! `epdf.c`, `lookup_fontmap`): a Type 1 font embedded in the file is
//! replaced by the map's font file, subset to the glyphs of the file's
//! `/CharSet`. The font's encoding is xpdf's (`Gfx8BitFont`): the base
//! encoding of `/Encoding` or of the embedded font program
//! (`FoFiType1`), else `StandardEncoding`, with `/Differences` merged.

use std::collections::BTreeSet;
use std::sync::Arc;

use phitex_codecs::pdfread::{Dict, Doc, Obj, Ref};

use crate::encodings::{MAC_EXPERT, MAC_ROMAN, STANDARD, WIN_ANSI};
use crate::map::MapEntry;
use crate::{FontSource, PdfError, PdfState};

/// An encoding: 256 glyph names, `None` for none.
pub(crate) type Names = Vec<Option<Vec<u8>>>;

/// A font descriptor made for an included file's font
/// (`epdf_create_fontdescriptor`), kept until the fonts are written.
#[derive(Clone, Debug)]
pub(crate) struct EpdfFd {
    pub fm: MapEntry,
    /// `fd_objnum`, given when the descriptor is made.
    pub objnum: usize,
    /// `fn_objnum`, given when a font dictionary refers to it.
    pub fn_objnum: usize,
    /// `/StemV` of the file's descriptor, rounded.
    pub stemv: i32,
    /// `gl_tree`: the glyphs of the files' `/CharSet`s.
    pub glyphs: BTreeSet<Vec<u8>>,
    /// `all_glyphs`.
    pub all_glyphs: bool,
    /// The font file.
    pub file: Option<Arc<[u8]>>,
}

/// The key of `fd_tree`: font file, slant, extend.
pub(crate) type FdKey = (Vec<u8>, i32, i32);

fn table(t: &[Option<&str>; 256]) -> Names {
    t.iter().map(|n| n.map(|s| s.as_bytes().to_vec())).collect()
}

/// `FoFiType1::undoPFB`: the segments of a PFB file joined.
// xpdf FoFiType1.cc
fn undo_pfb(file: &[u8]) -> Vec<u8> {
    if file.first() != Some(&0x80) {
        return file.to_vec();
    }
    let mut out = Vec::new();
    let mut pos = 0;
    while pos + 6 <= file.len() && file[pos] == 0x80 && matches!(file[pos + 1], 1 | 2) {
        let n = u32::from_le_bytes([file[pos + 2], file[pos + 3], file[pos + 4], file[pos + 5]])
            as usize;
        let end = (pos + 6 + n).min(file.len());
        out.extend_from_slice(&file[pos + 6..end]);
        pos = end;
    }
    out
}

/// `FoFiType1::getNextLine`.
// xpdf FoFiType1.cc
fn next_line(f: &[u8], mut line: usize) -> Option<usize> {
    while line < f.len() && f[line] != b'\n' && f[line] != b'\r' {
        line += 1;
    }
    if line < f.len() && f[line] == b'\r' {
        line += 1;
    }
    if line < f.len() && f[line] == b'\n' {
        line += 1;
    }
    (line < f.len()).then_some(line)
}

/// A C string's view of `buf`: up to its first NUL.
fn cstr(buf: &[u8]) -> &[u8] {
    &buf[..buf.iter().position(|&c| c == 0).unwrap_or(buf.len())]
}

/// `FoFiType1::parse`'s encoding: `None` when the program has none.
// xpdf FoFiType1.cc
#[allow(clippy::too_many_lines)]
fn fofi_type1_encoding(file: &[u8]) -> Option<Names> {
    let f = undo_pfb(file);
    let f = &f[..];
    let mut name = false;
    let mut encoding: Option<Names> = None;
    let mut got_matrix = false;
    let mut line = Some(0usize);
    let mut i = 1;
    while i <= 100 && (!name || encoding.is_none() || !got_matrix) {
        let Some(l) = line else {
            break;
        };
        let rest = &f[l..];
        if !name && rest.len() >= 9 && rest.starts_with(b"/FontName") {
            let buf = cstr(&rest[..rest.len().min(255)]);
            if let Some(p) = buf[9.min(buf.len())..].iter().position(|&c| c == b'/') {
                let after = &buf[9 + p + 1..];
                if after
                    .iter()
                    .any(|c| !matches!(c, b' ' | b'\t' | b'\n' | b'\r'))
                {
                    name = true;
                }
            }
            line = next_line(f, l);
        } else if encoding.is_none()
            && rest.len() >= 30
            && rest.starts_with(b"/Encoding StandardEncoding def")
        {
            encoding = Some(table(&STANDARD));
        } else if encoding.is_none() && rest.len() >= 19 && rest.starts_with(b"/Encoding 256 array")
        {
            let mut enc: Names = vec![None; 256];
            let mut j = 0;
            line = next_line(f, l);
            while j < 300 {
                let Some(cur) = line else {
                    break;
                };
                let Some(line1) = next_line(f, cur) else {
                    break;
                };
                let n = (line1 - cur).min(255);
                let buf = cstr(&f[cur..cur + n]).to_vec();
                let mut p = buf.iter().take_while(|&&c| c == b' ' || c == b'\t').count();
                let ends_with_dup = (cur >= 4 && &f[cur - 4..cur] == b"dup\n")
                    || (cur >= 5 && &f[cur - 5..cur - 1] == b"dup\r");
                let starts_with_dup = buf[p..].starts_with(b"dup");
                let at = |p: usize| buf.get(p).copied().unwrap_or(0);
                if ends_with_dup || starts_with_dup {
                    if starts_with_dup {
                        p += 3;
                    }
                    loop {
                        while matches!(at(p), b' ' | b'\t') {
                            p += 1;
                        }
                        let mut code: u32 = 0;
                        let base = if at(p) == b'8' && at(p + 1) == b'#' {
                            p += 2;
                            8
                        } else if at(p).is_ascii_digit() {
                            10
                        } else {
                            break;
                        };
                        while at(p) >= b'0' && u32::from(at(p)) < u32::from(b'0') + base {
                            code = code
                                .wrapping_mul(base)
                                .wrapping_add(u32::from(at(p) - b'0'));
                            p += 1;
                        }
                        while matches!(at(p), b' ' | b'\t') {
                            p += 1;
                        }
                        if at(p) != b'/' {
                            break;
                        }
                        p += 1;
                        let mut p2 = p;
                        while p2 < buf.len() && !matches!(buf[p2], b' ' | b'\t') {
                            p2 += 1;
                        }
                        if code < 256 {
                            enc[code as usize] = Some(buf[p..p2].to_vec());
                        }
                        p = p2;
                        while matches!(at(p), b' ' | b'\t') {
                            p += 1;
                        }
                        if !buf[p.min(buf.len())..].starts_with(b"put") {
                            break;
                        }
                        p += 3;
                        while matches!(at(p), b' ' | b'\t') {
                            p += 1;
                        }
                        if !buf[p.min(buf.len())..].starts_with(b"dup") {
                            break;
                        }
                        p += 3;
                    }
                } else {
                    // strtok(buf, " \t"), then the next token to " \t\n\r"
                    let mut toks = buf
                        .split(|c| matches!(c, b' ' | b'\t'))
                        .filter(|t| !t.is_empty());
                    if toks.next().is_some() {
                        let rest: Vec<u8> = toks.flat_map(|t| [t, b" "].concat()).collect();
                        let second = rest
                            .split(|c| matches!(c, b' ' | b'\t' | b'\n' | b'\r'))
                            .find(|t| !t.is_empty());
                        if second == Some(b"def") {
                            break;
                        }
                    }
                }
                j += 1;
                line = Some(line1);
            }
            encoding = Some(enc);
        } else if !got_matrix && rest.len() >= 11 && rest.starts_with(b"/FontMatrix") {
            got_matrix = true;
        } else {
            line = next_line(f, l);
        }
        i += 1;
    }
    encoding
}

/// `Gfx8BitFont`'s `enc` for a Type 1 font with an embedded program.
// xpdf GfxFont.cc, "build the font encoding"
pub(crate) fn char_names(doc: &Doc, font: &Dict, font_file: Option<&[u8]>) -> Names {
    let enc_obj = doc.lookup(font, b"Encoding");
    let named = |n: &[u8]| -> Option<Names> {
        match n {
            b"MacRomanEncoding" => Some(table(&MAC_ROMAN)),
            b"MacExpertEncoding" => Some(table(&MAC_EXPERT)),
            b"WinAnsiEncoding" => Some(table(&WIN_ANSI)),
            _ => None,
        }
    };
    let mut base = match &enc_obj {
        Obj::Dict(d) => doc.lookup(d, b"BaseEncoding").as_name().and_then(named),
        Obj::Name(n) => named(n),
        _ => None,
    };
    if base.is_none()
        && let Some(f) = font_file
    {
        base = fofi_type1_encoding(f);
    }
    let mut enc = base.unwrap_or_else(|| table(&STANDARD));
    if let Obj::Dict(d) = &enc_obj
        && let Obj::Array(a) = doc.lookup(d, b"Differences")
    {
        let mut code: i64 = 0;
        for x in &a {
            match doc.follow(x) {
                Obj::Int(i) => code = i64::from(i),
                Obj::Name(n) => {
                    if let Ok(c) = usize::try_from(code)
                        && c < 256
                    {
                        enc[c] = Some(n.clone());
                    }
                    code += 1;
                }
                _ => {}
            }
        }
    }
    enc
}

/// `lookup_fontmap`'s name: the subset tag cut, `-Slant_<n>` and
/// `-Extend_<n>` read off.
fn ps_key(ps: &[u8]) -> (Vec<u8>, i32, i32) {
    let mut s = ps;
    if ps.len() > 7 && ps[..6].iter().all(u8::is_ascii_uppercase) && ps[6] == b'+' {
        s = &ps[7..];
    }
    let find = |h: &[u8], n: &[u8]| h.windows(n.len()).position(|w| w == n);
    // strtol: digits with an optional sign, the whole rest
    let num = |b: &[u8]| -> Option<(i32, usize)> {
        let mut i = 0;
        while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C) {
            i += 1;
        }
        let st = i;
        if i < b.len() && matches!(b[i], b'+' | b'-') {
            i += 1;
        }
        let d = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == d {
            return None;
        }
        std::str::from_utf8(&b[st..i])
            .ok()?
            .parse()
            .ok()
            .map(|v| (v, i))
    };
    if let Some(a) = find(s, b"-Slant_") {
        let b = &s[a + 7..];
        if let Some((sl, e)) = num(b) {
            if e == b.len() {
                return (s[..a].to_vec(), sl, 0);
            }
            if let Some(c) = find(&b[e..], b"-Extend_") {
                let d = &b[e + c + 8..];
                if let Some((ex, e2)) = num(d)
                    && e2 == d.len()
                {
                    return (s[..a].to_vec(), sl, ex);
                }
            }
        }
    } else if let Some(a) = find(s, b"-Extend_") {
        let b = &s[a + 8..];
        if let Some((ex, e)) = num(b)
            && e == b.len()
        {
            return (s[..a].to_vec(), 0, ex);
        }
    }
    (s.to_vec(), 0, 0)
}

/// `zround`.
#[allow(clippy::cast_possible_truncation)]
fn zround(r: f64) -> i32 {
    if r > 2_147_483_647.0 {
        i32::MAX
    } else if r < -2_147_483_647.0 {
        -i32::MAX
    } else if r >= 0.0 {
        (r + 0.5) as i32
    } else {
        (r - 0.5) as i32
    }
}

/// The map entry of a font replaced: its key and the entry.
pub(crate) struct Replacement {
    pub fm: MapEntry,
    pub stemv: i32,
    pub charset: Option<Vec<u8>>,
    pub desc_ref: Ref,
    pub names: Names,
    pub file: Option<Arc<[u8]>>,
}

impl PdfState {
    /// `lookup_fontmap`: the map entry (`\pdfmapline`s first, then the
    /// default map) of a Type 1 font with a font file, by PostScript name
    /// (`ps_tree`), if its font file exists.
    pub(crate) fn lookup_fontmap(
        &self,
        ps: &[u8],
        src: &dyn FontSource,
    ) -> Option<(MapEntry, Option<Arc<[u8]>>)> {
        let (name, slant, extend) = ps_key(ps);
        let ok = |m: &MapEntry| {
            m.ps_name.as_deref() == Some(&name[..])
                && m.slant == slant
                && m.extend == extend
                && m.is_type1()
                && m.is_fontfile()
                && m.is_included()
        };
        let fm = self
            .map_over
            .values()
            .flatten()
            .find(|m| ok(m))
            .cloned()
            .or_else(|| src.map_by_ps_name(&name, slant, extend))
            .or_else(|| self.map_add.values().find(|m| ok(m)).cloned())?;
        let file = src.file(fm.ff_name.as_deref().unwrap_or_default())?;
        Some((fm, Some(file.into())))
    }

    /// `copyFont`'s test: whether font `r` of the included file is
    /// replaced, and with what.
    pub(crate) fn replacement(
        &self,
        doc: &Doc,
        r: Ref,
        src: &dyn FontSource,
    ) -> Result<Option<Replacement>, PdfError> {
        if self.fixed_inclusion_copy_font != 0 {
            return Ok(None);
        }
        let Obj::Dict(font) = doc.fetch(r) else {
            return Ok(None);
        };
        if doc.lookup(&font, b"Subtype").as_name() != Some(b"Type1") {
            return Ok(None);
        }
        let Some(base) = doc.lookup(&font, b"BaseFont").as_name().map(<[u8]>::to_vec) else {
            return Ok(None);
        };
        let Some(Obj::Ref(desc_ref)) = font.get(b"FontDescriptor") else {
            return Ok(None);
        };
        let desc_ref = *desc_ref;
        let Obj::Dict(desc) = doc.fetch(desc_ref) else {
            return Ok(None);
        };
        let file1 = match doc.lookup(&desc, b"FontFile") {
            Obj::Stream(s) => Some(doc.decode(&s)),
            _ => None,
        };
        if file1.is_none() {
            let t1c = matches!(doc.lookup(&desc, b"FontFile3"),
                Obj::Stream(s) if doc.lookup(&s.dict, b"Subtype").as_name() == Some(b"Type1C"));
            if !t1c {
                return Ok(None);
            }
        }
        let Some((fm, file)) = self.lookup_fontmap(&base, src) else {
            return Ok(None);
        };
        if file1.is_none() {
            return Err(PdfError::Unsupported(
                "replacing an included PDF's Type 1C font".into(),
            ));
        }
        let stemv = zround(doc.lookup(&desc, b"StemV").as_num().unwrap_or(0.0));
        let charset = match doc.lookup(&desc, b"CharSet") {
            Obj::Str(s) => Some(s),
            _ => None,
        };
        let names = char_names(doc, &font, file1.as_deref());
        Ok(Some(Replacement {
            fm,
            stemv,
            charset,
            desc_ref,
            names,
            file,
        }))
    }

    /// `epdf_create_fontdescriptor`, `epdf_mark_glyphs` and
    /// `embed_whole_font`: the descriptor's key and number.
    pub(crate) fn epdf_fd(&mut self, rep: &Replacement) -> (FdKey, usize) {
        let fm = &rep.fm;
        let key = (fm.ff_name.clone().unwrap_or_default(), fm.slant, fm.extend);
        if !self.epdf_fds.contains_key(&key) {
            let objnum = self.new_objnum_();
            self.epdf_fds.insert(
                key.clone(),
                EpdfFd {
                    fm: fm.clone(),
                    objnum,
                    fn_objnum: 0,
                    stemv: rep.stemv,
                    glyphs: BTreeSet::new(),
                    all_glyphs: false,
                    file: rep.file.clone(),
                },
            );
        }
        let e = self.epdf_fds.get_mut(&key).expect("inserted above");
        match &rep.charset {
            Some(cs) if fm.is_subsetted() => {
                // epdf_mark_glyphs
                let sp = |c: &u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0C);
                let cs = cstr(cs);
                let start = cs.iter().take_while(|c| sp(c)).count();
                let cs = &cs[start..];
                let mut s = 1;
                while s < cs.len() {
                    let mut p = s;
                    while p < cs.len() && cs[p] != b'/' && !sp(&cs[p]) {
                        p += 1;
                    }
                    let glyph = cs[s..p].to_vec();
                    if p < cs.len() && sp(&cs[p]) {
                        p += 1;
                        while p < cs.len() && sp(&cs[p]) {
                            p += 1;
                        }
                    }
                    e.glyphs.insert(glyph);
                    s = p + 1;
                }
            }
            _ => e.all_glyphs = true,
        }
        (key.clone(), e.objnum)
    }

    /// `get_fn_objnum`.
    pub(crate) fn epdf_fn_objnum(&mut self, key: &FdKey) -> usize {
        let n = self.epdf_fds.get(key).map_or(0, |e| e.fn_objnum);
        if n != 0 {
            return n;
        }
        let n = self.new_objnum_();
        if let Some(e) = self.epdf_fds.get_mut(key) {
            e.fn_objnum = n;
        }
        n
    }

    /// `epdf_write_enc`.
    pub(crate) fn epdf_write_enc(&mut self, names: &Names, objnum: usize) {
        self.begin_dict(objnum, 1);
        self.print(b"/Type /Encoding\n");
        self.print(b"/Differences [");
        let mut i_old: i64 = -2;
        for (i, n) in names.iter().enumerate() {
            let Some(n) = n else {
                continue;
            };
            let i = i64::try_from(i).unwrap_or(0);
            if i == i_old + 1 {
                self.print(b"/");
            } else if i_old == -2 {
                self.print(format!("{i}/").as_bytes());
            } else {
                self.print(format!(" {i}/").as_bytes());
            }
            self.print(n);
            i_old = i;
        }
        self.print(b"]\n");
        self.end_dict();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        assert_eq!(ps_key(b"ABCDEF+CMR10"), (b"CMR10".to_vec(), 0, 0));
        assert_eq!(ps_key(b"Foo-Slant_167"), (b"Foo".to_vec(), 167, 0));
        assert_eq!(
            ps_key(b"Foo-Slant_167-Extend_900"),
            (b"Foo".to_vec(), 167, 900)
        );
        assert_eq!(ps_key(b"Foo-Extend_900"), (b"Foo".to_vec(), 0, 900));
    }

    #[test]
    fn encoding_of_a_program() {
        let f = b"%!PS\n/FontName /X def\n/Encoding 256 array\n0 1 255 {1 index exch /.notdef put} for\ndup 73 /I put\ndup 97 /a put\nreadonly def\n/FontMatrix [0.001 0 0 0.001 0 0] def\n";
        let e = fofi_type1_encoding(f).unwrap();
        assert_eq!(e[73].as_deref(), Some(&b"I"[..]));
        assert_eq!(e[97].as_deref(), Some(&b"a"[..]));
        assert_eq!(e.iter().flatten().count(), 2);
    }
}
