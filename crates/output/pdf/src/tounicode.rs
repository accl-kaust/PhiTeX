//! `tounicode.c`: `\pdfglyphtounicode` entries and the `/ToUnicode`
//! `CMap`s of `\pdfgentounicode`, from glyph names resolved through
//! those entries and the `uniXXXX` and `uXXXX[XX]` conventions.

use std::collections::BTreeMap;

use crate::PdfState;
use crate::enc::GlyphNames;

/// A glyph's Unicode: a code point, a UTF-16BE hex string
/// (`UNI_STRING`, `UNI_EXTRA_STRING`), or none (`UNI_UNDEF`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Uni {
    Undef,
    Code(u32),
    Str(Vec<u8>),
}

/// `glyph_unicode_tree`.
type Tree = BTreeMap<Vec<u8>, Uni>;

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

/// `set_glyph_unicode`.
fn glyph_unicode(s: &[u8], tfm: &[u8], tree: &Tree) -> Uni {
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
        return e.clone();
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

/// [`cmap`], memoized by its inputs (`tree` by its content key,
/// [`crate::digest::TMap::content_key`]): the end of the file is written
/// again after any change that reaches it, its fonts' `CMap`s with it,
/// most of them made from the same glyph names as the last time.
fn cmap_memo(names: &GlyphNames, tfm: &[u8], name: &[u8], tree: &Tree, tree_key: (u128, usize)) -> Vec<u8> {
    use std::hash::{Hash, Hasher};
    use std::sync::{Mutex, OnceLock, PoisonError};
    type Memo = Mutex<std::collections::HashMap<(u64, u64, usize), std::sync::Arc<[u8]>>>;
    static MEMO: OnceLock<Memo> = OnceLock::new();
    let h = |seed: u64| {
        let mut s = std::hash::DefaultHasher::new();
        seed.hash(&mut s);
        (names, tfm, name, tree_key).hash(&mut s);
        s.finish()
    };
    let key = (h(0), h(0x9e37_79b9_7f4a_7c15), names.len());
    let memo = MEMO.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    if let Some(t) = memo.lock().unwrap_or_else(PoisonError::into_inner).get(&key) {
        return t.to_vec();
    }
    let t = cmap(names, tfm, name, tree);
    let mut m = memo.lock().unwrap_or_else(PoisonError::into_inner);
    if m.len() > 4096 {
        m.clear();
    }
    m.insert(key, t.as_slice().into());
    t
}

/// The `CMap` stream's text for `names` (`write_tounicode` from the
/// header on).
#[allow(clippy::too_many_lines)]
fn cmap(names: &GlyphNames, tfm: &[u8], name: &[u8], tree: &Tree) -> Vec<u8> {
    let b = String::from_utf8_lossy(name);
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
                .and_then(Option::as_deref)
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

impl PdfState {
    /// `\pdfglyphtounicode{glyph}{unistr}` (`deftounicode`): ignored
    /// when invalid (a warning).
    pub fn glyph_to_unicode(&mut self, glyph: &[u8], unistr: &[u8]) {
        let glyph = glyph.split(|&c| c == 0).next().unwrap_or_default();
        let p = unistr.split(|&c| c == 0).next().unwrap_or_default();
        let a = p.iter().position(|&c| c != b' ').unwrap_or(p.len());
        let p = &p[a..];
        let mut l = p.len();
        while l > 0 && p[l - 1] == b' ' {
            l -= 1;
        }
        let mut valid = 1;
        for &c in &p[..l] {
            if c == b' ' {
                valid = 2;
            } else if !c.is_ascii_hexdigit() {
                valid = 0;
                break;
            }
        }
        if l == 0 || valid == 0 || glyph.is_empty() || glyph == b".notdef" {
            return;
        }
        let v = if valid == 2 {
            // the whole text without spaces (trailing ones included)
            Uni::Str(p.iter().copied().filter(|&c| c != b' ').collect())
        } else {
            // `sscanf("%lX")`: the leading hex digits
            let mut v: u64 = 0;
            for &c in p.iter().take_while(|c| c.is_ascii_hexdigit()) {
                v = v
                    .saturating_mul(16)
                    .saturating_add(u64::from(char::from(c).to_digit(16).unwrap_or(0)));
            }
            u32::try_from(v)
                .ok()
                .filter(|&v| v <= 0x10_FFFF)
                .map_or(Uni::Undef, Uni::Code)
        };
        self.glyph_unicode.insert(glyph.to_vec(), v);
    }

    /// `write_tounicode`: the `CMap` object, or 0 (with
    /// `\pdfgentounicode` then off) without `\pdfglyphtounicode` entries.
    pub(crate) fn write_tounicode(
        &mut self,
        names: &GlyphNames,
        tfm: &[u8],
        enc: Option<&[u8]>,
    ) -> usize {
        if self.glyph_unicode.is_empty() {
            self.fixed_gen_tounicode = 0;
            return 0;
        }
        let (name, _) = cmap_name(tfm, enc);
        let tree = self.glyph_unicode.content_key();
        let text = cmap_memo(names, tfm, &name, &self.glyph_unicode, tree);
        let objnum = self.new_objnum_();
        self.begin_dict(objnum, 0);
        self.begin_stream();
        self.print(&text);
        self.end_stream();
        objnum
    }
}
