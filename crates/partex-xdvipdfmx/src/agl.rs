//! agl.c, agl.h: the Adobe Glyph List (glyph name to Unicode).
//!
//! The map is `self.agl.aglmap`, an [`HtTable`] of [`AglName`]s; a name
//! listed twice chains the later entries through `alternate`.

use crate::dpxutil::HtTable;
use crate::prelude::*;

/// `AGL_DEFAULT_LISTFILE`.
pub const AGL_DEFAULT_LISTFILE: &[u8] = b"glyphlist.txt";
/// `AGL_PREDEF_LISTFILE`.
pub const AGL_PREDEF_LISTFILE: &[u8] = b"pdfglyphlist.txt";
/// `AGL_EXTRA_LISTFILE`.
pub const AGL_EXTRA_LISTFILE: &[u8] = b"texglyphlist.txt";
/// `AGL_MAX_UNICODES`.
pub const AGL_MAX_UNICODES: usize = 16;
/// `SUFFIX_LIST_MAX`.
pub const SUFFIX_LIST_MAX: usize = 16;
/// `AGL_VAR_SMCP_IDX`.
pub const AGL_VAR_SMCP_IDX: i32 = 0;
/// `WBUF_SIZE`.
pub const WBUF_SIZE: usize = 1024;

/// `agl_name`.
#[derive(Clone, Debug, Default)]
pub struct AglName {
    pub name: Option<Vec<u8>>,
    pub suffix: Option<Vec<u8>>,
    pub n_components: i32,
    pub unicodes: [i32; AGL_MAX_UNICODES],
    /// The next entry for the same name.
    pub alternate: Option<Box<AglName>>,
    pub is_predef: i32,
}

/// agl.c's statics.
#[derive(Clone, Debug, Default)]
pub struct State {
    /// `aglmap`.
    pub aglmap: HtTable<AglName>,
}

/// `modifiers` (static; without the C terminator).
pub static MODIFIERS: [&[u8]; 20] = [
    b"acute",
    b"breve",
    b"caron",
    b"cedilla",
    b"circumflex",
    b"dieresis",
    b"dotaccent",
    b"grave",
    b"hungarumlaut",
    b"macron",
    b"ogonek",
    b"ring",
    b"tilde",
    b"commaaccent",
    b"slash",
    b"ampersand",
    b"exclam",
    b"exclamdown",
    b"question",
    b"questiondown",
];

/// An entry of `var_list`: key, OTL tag, suffixes.
#[derive(Clone, Copy, Debug)]
pub struct VarListEntry {
    pub key: &'static [u8],
    pub otl_tag: Option<&'static [u8]>,
    pub suffixes: &'static [&'static [u8]],
}

/// `var_list` (static; without the C terminator).
pub static VAR_LIST: [VarListEntry; 13] = [
    VarListEntry {
        key: b"small",
        otl_tag: Some(b"smcp"),
        suffixes: &[b"sc"],
    },
    VarListEntry {
        key: b"swash",
        otl_tag: Some(b"swsh"),
        suffixes: &[],
    },
    VarListEntry {
        key: b"superior",
        otl_tag: Some(b"sups"),
        suffixes: &[],
    },
    VarListEntry {
        key: b"inferior",
        otl_tag: Some(b"sinf"),
        suffixes: &[],
    },
    VarListEntry {
        key: b"numerator",
        otl_tag: Some(b"numr"),
        suffixes: &[],
    },
    VarListEntry {
        key: b"denominator",
        otl_tag: Some(b"dnom"),
        suffixes: &[],
    },
    VarListEntry {
        key: b"oldstyle",
        otl_tag: Some(b"onum"),
        suffixes: &[],
    },
    VarListEntry {
        key: b"display",
        otl_tag: None,
        suffixes: &[],
    },
    VarListEntry {
        key: b"text",
        otl_tag: None,
        suffixes: &[],
    },
    VarListEntry {
        key: b"big",
        otl_tag: None,
        suffixes: &[],
    },
    VarListEntry {
        key: b"bigg",
        otl_tag: None,
        suffixes: &[],
    },
    VarListEntry {
        key: b"Big",
        otl_tag: None,
        suffixes: &[],
    },
    VarListEntry {
        key: b"Bigg",
        otl_tag: None,
        suffixes: &[],
    },
];

/// `agl_new_name` (static).
fn agl_new_name() -> AglName {
    AglName {
        name: None,
        suffix: None,
        n_components: 0,
        unicodes: [0; AGL_MAX_UNICODES],
        alternate: None,
        is_predef: 0,
    }
}

/// `strchr(s, c)` on a C string held as a slice: the index.
fn strchr(s: &[u8], c: u8) -> Option<usize> {
    s.iter().position(|&b| b == c)
}

/// `agl_chop_suffix`: the name (none when the glyph name starts with
/// `.`) and the suffix.
pub fn agl_chop_suffix(glyphname: &[u8]) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    match strchr(glyphname, b'.') {
        Some(len) => {
            if len < 1 {
                (None, Some(glyphname[1..].to_vec()))
            } else {
                let p = &glyphname[len + 1..];
                let name = glyphname[..len].to_vec();
                if p.is_empty() {
                    (Some(name), None)
                } else {
                    (Some(name), Some(p.to_vec()))
                }
            }
        }
        None => (Some(glyphname.to_vec()), None),
    }
}

/// `skip_capital` (static).
fn skip_capital(s: &[u8], p: &mut usize) -> i32 {
    let q = &s[*p..];
    let len = q.len();
    let slen;
    if len >= 2 && ((q[0] == b'A' && q[1] == b'E') || (q[0] == b'O' && q[1] == b'E')) {
        slen = 2;
    } else if len >= 3 && q.starts_with(b"Eth") {
        slen = 3;
    } else if len >= 5 && q.starts_with(b"Thorn") {
        slen = 5;
    } else if len >= 1 && q[0].is_ascii_uppercase() {
        slen = 1;
    } else {
        slen = 0;
    }
    *p += slen;
    slen as i32
}

/// `skip_modifier` (static). C compares the whole rest (`memcmp` over
/// `len` bytes, the modifier's NUL included), so only a rest that is
/// exactly a modifier matches.
fn skip_modifier(s: &[u8], p: &mut usize) -> i32 {
    let q = &s[*p..];
    let mut slen = 0;
    for m in &MODIFIERS {
        if q.len() >= m.len() && q == *m {
            slen = m.len();
            *p += slen;
            break;
        }
    }
    slen as i32
}

/// `is_smallcap` (static).
fn is_smallcap(glyphname: &[u8]) -> bool {
    let mut len = glyphname.len() as i32;
    if len < 6 || !glyphname.ends_with(b"small") {
        return false;
    }
    let s = &glyphname[..glyphname.len() - 5];
    let mut p = 0;
    len -= 5;
    let mut slen = skip_modifier(s, &mut p);
    if slen == len {
        return true;
    } else if slen > 0 {
        return false;
    }
    len -= skip_capital(s, &mut p);
    if len == 0 {
        return true;
    }
    while len > 0 {
        slen = skip_modifier(s, &mut p);
        if slen == 0 {
            return false;
        }
        len -= slen;
    }
    true
}

/// `agl_suffix_to_otltag`.
#[must_use]
pub fn agl_suffix_to_otltag(suffix: &[u8]) -> Option<&'static [u8]> {
    for v in &VAR_LIST {
        for &sfx in v.suffixes {
            if suffix == sfx {
                return v.otl_tag;
            }
        }
        if suffix == v.key {
            return v.otl_tag;
        }
        if let Some(tag) = v.otl_tag
            && suffix == tag
        {
            return v.otl_tag;
        }
    }
    None
}

/// `agl_guess_name` (static): an index into `VAR_LIST`, or -1.
fn agl_guess_name(glyphname: &[u8]) -> i32 {
    if is_smallcap(glyphname) {
        return AGL_VAR_SMCP_IDX;
    }
    let len = glyphname.len();
    for i in 1..VAR_LIST.len() {
        let key = VAR_LIST[i].key;
        if len > key.len() && glyphname.ends_with(key) {
            return i as i32;
        }
    }
    -1
}

/// `agl_normalized_name` (static).
fn agl_normalized_name(glyphname: &[u8]) -> Option<AglName> {
    let mut agln = agl_new_name();
    if let Some(n) = strchr(glyphname, b'.') {
        if n + 1 < glyphname.len() {
            agln.suffix = Some(glyphname[n + 1..].to_vec());
        }
        agln.name = Some(glyphname[..n].to_vec());
    } else if is_smallcap(glyphname) {
        let n = glyphname.len() - 5;
        agln.suffix = Some(b"sc".to_vec());
        agln.name = Some(
            glyphname[..n]
                .iter()
                .map(|&c| if c.is_ascii_uppercase() { c + 32 } else { c })
                .collect(),
        );
    } else {
        let var_idx = agl_guess_name(glyphname);
        let n;
        if var_idx < 0 {
            n = glyphname.len();
        } else {
            let v = &VAR_LIST[var_idx as usize];
            n = glyphname.len() - v.key.len();
            if let Some(&s0) = v.suffixes.first() {
                agln.suffix = Some(s0.to_vec());
            } else {
                agln.suffix = Some(v.key.to_vec());
            }
        }
        agln.name = Some(glyphname[..n].to_vec());
    }
    Some(agln)
}

/// `agl_name_is_unicode`: `uniXXXX` or `uXXXX[XX]`.
#[must_use]
pub fn agl_name_is_unicode(glyphname: &[u8]) -> bool {
    let len = strchr(glyphname, b'.').unwrap_or(glyphname.len());
    let is_hex = |c: u8| c.is_ascii_digit() || (b'A'..=b'F').contains(&c);
    if len >= 7 && (len - 3) % 4 == 0 && glyphname.starts_with(b"uni") {
        is_hex(glyphname[3])
    } else if (5..=7).contains(&len) && glyphname[0] == b'u' {
        for i in 1..len - 1 {
            if !is_hex(glyphname[i]) {
                return false;
            }
        }
        true
    } else {
        false
    }
}

/// `agl_name_convert_unicode`: the code point, or -1.
#[must_use]
pub fn agl_name_convert_unicode(glyphname: &[u8]) -> i32 {
    if !agl_name_is_unicode(glyphname) {
        return -1;
    }
    if glyphname.len() > 7 && glyphname[7] != b'.' {
        warn!("Mapping to multiple Unicode characters not supported.");
        return -1;
    }
    let mut p = if glyphname[1] == b'n' { 3 } else { 1 };
    let mut ucv: i32 = 0;
    while p < glyphname.len() && glyphname[p] != b'.' {
        let c = glyphname[p];
        if !c.is_ascii_digit() && !(b'A'..=b'F').contains(&c) {
            warn!("Invalid char in Unicode glyph name.");
            return -1;
        }
        ucv = ucv.wrapping_shl(4);
        ucv = ucv.wrapping_add(if c.is_ascii_digit() {
            i32::from(c - b'0')
        } else {
            i32::from(c - b'A') + 10
        });
        p += 1;
    }
    if !crate::unicode::UC_is_valid(ucv) {
        warn!("Invalid Unicode code value U+{:04X}.", ucv);
        ucv = -1;
    }
    ucv
}

/// `xtol` (static): `len` bytes of `s` (past its end, C's NUL).
fn xtol(s: &[u8], len: i32) -> i32 {
    let mut v: i32 = 0;
    for i in 0..len.max(0) as usize {
        let c = s.get(i).copied().unwrap_or(0);
        v = v.wrapping_shl(4);
        if c.is_ascii_digit() {
            v = v.wrapping_add(i32::from(c - b'0'));
        } else if (b'A'..=b'F').contains(&c) {
            v = v.wrapping_add(i32::from(c - b'A') + 10);
        } else {
            return -1;
        }
    }
    v
}

/// `IS_PUA`.
fn is_pua(u: i32) -> bool {
    (0x00E000..=0x00F8FF).contains(&u)
        || (0x0F0000..=0x0FFFFD).contains(&u)
        || (0x100000..=0x10FFFD).contains(&u)
}

/// `put_unicode_glyph` (static): the bytes written at `dst[*dstp..]`.
fn put_unicode_glyph(name: &[u8], dst: &mut [u8], dstp: &mut usize) -> i32 {
    let mut len: i32 = 0;
    if name[1] != b'n' {
        let p = &name[1..];
        let ucv = xtol(p, p.len() as i32);
        len += crate::unicode::UC_UTF16BE_encode_char(ucv, dst, dstp) as i32;
    } else {
        let mut p = 3;
        while p < name.len() {
            let ucv = xtol(&name[p..], 4);
            len += crate::unicode::UC_UTF16BE_encode_char(ucv, dst, dstp) as i32;
            p += 4;
        }
    }
    len
}

impl Dpx {
    /// `agl_init_map`.
    pub fn agl_init_map(&mut self) {
        self.agl.aglmap = HtTable::ht_init_table();
        self.agl_load_listfile(AGL_EXTRA_LISTFILE, 0);
        if self.agl_load_listfile(AGL_PREDEF_LISTFILE, 1) < 0 {
            warn!("Failed to load AGL file \"pdfglyphlist.txt\"...");
        }
        if self.agl_load_listfile(AGL_DEFAULT_LISTFILE, 0) < 0 {
            warn!("Failed to load AGL file \"glyphlist.txt\"...");
        }
    }

    /// `agl_close_map`.
    pub fn agl_close_map(&mut self) {
        self.agl.aglmap.ht_clear_table();
    }

    /// `agl_load_listfile` (static): the number of entries read, or -1.
    fn agl_load_listfile(&mut self, filename: &[u8], is_predef: i32) -> i32 {
        use crate::parse::{parse_ident, skip_white};
        let mut count = 0;
        let Some(mut fp) = self.dpx_open_file(filename, crate::dpxfile::ResType::Agl) else {
            return -1;
        };
        while let Some(mut line) = fp.mfgets(WBUF_SIZE) {
            // C works on the NUL-terminated buffer.
            if let Some(z) = strchr(&line, 0) {
                line.truncate(z);
            }
            let s = &line[..];
            let endptr = s.len();
            let mut p = 0;
            skip_white(s, &mut p);
            if p >= endptr || s[p] == b'#' {
                continue;
            }
            let nextptr = match strchr(&s[p..], b';') {
                Some(i) if i != 0 => p + i,
                _ => continue,
            };
            let name = parse_ident(&s[..nextptr], &mut p);
            skip_white(s, &mut p);
            let Some(name) = name else {
                warn!("Invalid AGL entry");
                continue;
            };
            if p >= endptr || s[p] != b';' {
                warn!("Invalid AGL entry");
                continue;
            }
            p += 1;
            skip_white(s, &mut p);

            let mut n_unicodes = 0usize;
            let mut unicodes = [0i32; AGL_MAX_UNICODES];
            while p < endptr && (s[p].is_ascii_digit() || (b'A'..=b'F').contains(&s[p])) {
                if n_unicodes >= AGL_MAX_UNICODES {
                    warn!("Too many Unicode values");
                    break;
                }
                let (v, used) = crate::fmt::strtol(&s[p..], 16);
                unicodes[n_unicodes] = v as i32;
                n_unicodes += 1;
                p += used;
                skip_white(s, &mut p);
            }
            if n_unicodes == 0 {
                warn!("AGL entry ignored (no mapping)");
                continue;
            }

            let mut agln = agl_normalized_name(&name).unwrap();
            agln.is_predef = is_predef;
            agln.n_components = n_unicodes as i32;
            agln.unicodes[..n_unicodes].copy_from_slice(&unicodes[..n_unicodes]);

            match self.agl.aglmap.ht_lookup_table_mut(&name) {
                None => self.agl.aglmap.ht_append_table(&name, agln),
                Some(mut duplicate) => {
                    while duplicate.alternate.is_some() {
                        duplicate = duplicate.alternate.as_mut().unwrap();
                    }
                    duplicate.alternate = Some(Box::new(agln));
                }
            }
            count += 1;
        }
        count
    }

    /// `agl_lookup_list`: a copy of the entry (with its `alternate` chain).
    pub fn agl_lookup_list(&mut self, glyphname: &[u8]) -> Option<AglName> {
        self.agl.aglmap.ht_lookup_table(glyphname).cloned()
    }

    /// `agl_sput_UTF16BE`: the bytes written at `dst[*dstp..]` (`dst`
    /// ends at C's `limptr`) and the `fail_count`.
    #[allow(non_snake_case)]
    pub fn agl_sput_UTF16BE(
        &mut self,
        glyphstr: &[u8],
        dst: &mut [u8],
        dstp: &mut usize,
    ) -> (i32, i32) {
        let mut len: i32 = 0;
        let mut count = 0;
        let mut p = 0;
        let endptr = strchr(glyphstr, b'.').unwrap_or(glyphstr.len());
        while p < endptr {
            let delim = match strchr(&glyphstr[p..], b'_') {
                Some(0) => {
                    warn!("Invalid glyph name component.");
                    count += 1;
                    return (len, count);
                }
                Some(i) if p + i <= endptr => p + i,
                _ => endptr,
            };
            let name = &glyphstr[p..delim];
            if agl_name_is_unicode(name) {
                let sub_len = put_unicode_glyph(name, dst, dstp);
                if sub_len > 0 {
                    len += sub_len;
                } else {
                    count += 1;
                }
            } else {
                let map = &self.agl.aglmap;
                let mut agln1 = map.ht_lookup_table(name);
                if agln1.is_none_or(|a| a.n_components == 1 && is_pua(a.unicodes[0])) {
                    if let Some(agln0) = agl_normalized_name(name) {
                        agln1 = map.ht_lookup_table(agln0.name.as_deref().unwrap_or(b""));
                    }
                }
                if let Some(a) = agln1 {
                    for i in 0..a.n_components as usize {
                        len +=
                            crate::unicode::UC_UTF16BE_encode_char(a.unicodes[i], dst, dstp) as i32;
                    }
                } else {
                    count += 1;
                }
            }
            p = delim + 1;
        }
        (len, count)
    }

    /// `agl_get_unicodes`: the count put in `unicodes` (its length is
    /// `max_unicodes`), or -1.
    pub fn agl_get_unicodes(&mut self, glyphstr: &[u8], unicodes: &mut [i32]) -> i32 {
        let max_unicodes = unicodes.len() as i32;
        let mut count: i32 = 0;
        let mut p = 0;
        let endptr = strchr(glyphstr, b'.').unwrap_or(glyphstr.len());
        while p < endptr {
            let delim = match strchr(&glyphstr[p..], b'_') {
                Some(0) => {
                    warn!("Invalid glyph name component.");
                    return -1;
                }
                Some(i) if p + i <= endptr => p + i,
                _ => endptr,
            };
            let name = &glyphstr[p..delim];
            if agl_name_is_unicode(name) {
                if name[1] != b'n' {
                    if count >= max_unicodes {
                        return -1;
                    }
                    let q = &name[1..];
                    unicodes[count as usize] = xtol(q, q.len() as i32);
                    count += 1;
                } else {
                    let mut q = 3;
                    while q < name.len() {
                        if count >= max_unicodes {
                            return -1;
                        }
                        unicodes[count as usize] = xtol(&name[q..], 4);
                        count += 1;
                        q += 4;
                    }
                }
            } else {
                let map = &self.agl.aglmap;
                let mut agln1 = map.ht_lookup_table(name);
                if agln1.is_none_or(|a| a.n_components == 1 && is_pua(a.unicodes[0])) {
                    if let Some(agln0) = agl_normalized_name(name) {
                        agln1 = map.ht_lookup_table(agln0.name.as_deref().unwrap_or(b""));
                    }
                }
                if let Some(a) = agln1 {
                    if count + a.n_components > max_unicodes {
                        return -1;
                    }
                    for i in 0..a.n_components as usize {
                        unicodes[count as usize] = a.unicodes[i];
                        count += 1;
                    }
                } else {
                    return -1;
                }
            }
            p = delim + 1;
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(agl_name_is_unicode(b"uni0041"));
        assert!(agl_name_is_unicode(b"uni00410042.sc"));
        assert!(!agl_name_is_unicode(b"union"));
        assert!(agl_name_is_unicode(b"u1F600"));
        // C does not check the last character of `uXXXX`.
        assert!(agl_name_is_unicode(b"u004z"));
        assert_eq!(agl_name_convert_unicode(b"uni0041.sc"), 0x41);
        assert_eq!(agl_name_convert_unicode(b"uni00410042"), -1);
        assert_eq!(agl_name_convert_unicode(b"u1F600"), 0x1F600);
        assert_eq!(
            agl_chop_suffix(b"a.sc"),
            (Some(b"a".to_vec()), Some(b"sc".to_vec()))
        );
        assert_eq!(
            agl_chop_suffix(b".notdef"),
            (None, Some(b"notdef".to_vec()))
        );
        assert_eq!(agl_chop_suffix(b"a."), (Some(b"a".to_vec()), None));
    }

    #[test]
    fn normalized() {
        assert!(is_smallcap(b"Asmall"));
        assert!(is_smallcap(b"AEsmall"));
        // C's memcmp is case-sensitive: the "Acutesmall" of its comment fails.
        assert!(!is_smallcap(b"Acutesmall"));
        assert!(is_smallcap(b"acutesmall"));
        assert!(is_smallcap(b"Aacutesmall"));
        assert!(!is_smallcap(b"Aacutegravesmall"));
        let n = agl_normalized_name(b"Aacutesmall").unwrap();
        assert_eq!(n.name.as_deref(), Some(&b"aacute"[..]));
        assert_eq!(n.suffix.as_deref(), Some(&b"sc"[..]));
        let n = agl_normalized_name(b"onesuperior").unwrap();
        assert_eq!(n.name.as_deref(), Some(&b"one"[..]));
        assert_eq!(n.suffix.as_deref(), Some(&b"superior"[..]));
        let n = agl_normalized_name(b"f_i.alt").unwrap();
        assert_eq!(n.name.as_deref(), Some(&b"f_i"[..]));
        assert_eq!(n.suffix.as_deref(), Some(&b"alt"[..]));
        assert_eq!(agl_suffix_to_otltag(b"sc"), Some(&b"smcp"[..]));
        assert_eq!(agl_suffix_to_otltag(b"onum"), Some(&b"onum"[..]));
        assert_eq!(agl_suffix_to_otltag(b"big"), None);
    }

    #[test]
    fn unicode_glyph() {
        let mut b = [0u8; 8];
        let mut p = 0;
        assert_eq!(put_unicode_glyph(b"uni00410042", &mut b, &mut p), 4);
        assert_eq!(&b[..4], &[0, 0x41, 0, 0x42]);
        assert_eq!(xtol(b"00zz", 4), -1);
    }
}
