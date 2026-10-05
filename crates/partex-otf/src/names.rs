//! The `name` table's records, as FreeType reads them (`tt_face_load_name`:
//! in table order, empty and out-of-range strings dropped), and the
//! conversions XeTeX and FreeType apply to them.

use alloc::string::String;
use alloc::vec::Vec;

use crate::face::{rd_u16, sfnt_offset, table_directory};
use crate::tag;

/// One name record with its raw bytes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct NameRecord {
    pub platform: u16,
    pub encoding: u16,
    pub language: u16,
    pub name_id: u16,
    pub bytes: Vec<u8>,
}

/// Family (1).
pub const FAMILY: u16 = 1;
/// Subfamily (2).
pub const SUBFAMILY: u16 = 2;
/// Full name (4).
pub const FULL_NAME: u16 = 4;
/// PostScript name (6).
pub const POSTSCRIPT_NAME: u16 = 6;
/// Typographic family (16).
pub const TYPOGRAPHIC_FAMILY: u16 = 16;
/// Typographic subfamily (17).
pub const TYPOGRAPHIC_SUBFAMILY: u16 = 17;

/// The name ids the font index keeps.
pub const INDEXED_IDS: [u16; 9] = [
    FAMILY,
    SUBFAMILY,
    FULL_NAME,
    POSTSCRIPT_NAME,
    TYPOGRAPHIC_FAMILY,
    TYPOGRAPHIC_SUBFAMILY,
    MAC_FULL_NAME,
    WWS_FAMILY,
    WWS_SUBFAMILY,
];

/// Mac full name (18).
pub const MAC_FULL_NAME: u16 = 18;
/// WWS family (21).
pub const WWS_FAMILY: u16 = 21;
/// WWS subfamily (22).
pub const WWS_SUBFAMILY: u16 = 22;

/// The records of the `name` table `d` FreeType keeps, in table order,
/// those with an id in `ids` (all if empty).
#[must_use]
pub fn read_name_table(d: &[u8], ids: &[u16]) -> Vec<NameRecord> {
    let mut out = Vec::new();
    let (Some(format), Some(count), Some(storage)) = (rd_u16(d, 0), rd_u16(d, 2), rd_u16(d, 4))
    else {
        return out;
    };
    let mut storage_start = 6 + 12 * count as usize;
    let storage_limit = d.len();
    if storage_start > storage_limit {
        return out;
    }
    let mut lang_tags: Vec<u16> = Vec::new();
    if format == 1 {
        let n = rd_u16(d, storage_start).unwrap_or(0) as usize;
        let base = storage_start + 2;
        storage_start += 2 + 4 * n;
        for i in 0..n {
            let len = rd_u16(d, base + 4 * i).unwrap_or(0) as usize;
            let off = rd_u16(d, base + 4 * i + 2).unwrap_or(0) as usize + storage as usize;
            let ok = off >= storage_start && off + len <= storage_limit;
            lang_tags.push(if ok { len as u16 } else { 0 });
        }
    }
    for i in 0..count as usize {
        let r = 6 + 12 * i;
        let (Some(platform), Some(encoding), Some(language), Some(name_id), Some(len), Some(off)) = (
            rd_u16(d, r),
            rd_u16(d, r + 2),
            rd_u16(d, r + 4),
            rd_u16(d, r + 6),
            rd_u16(d, r + 8),
            rd_u16(d, r + 10),
        ) else {
            continue;
        };
        if len == 0 {
            continue;
        }
        let off = off as usize + storage as usize;
        if off < storage_start || off + len as usize > storage_limit {
            continue;
        }
        if format == 1 && language >= 0x8000 {
            let k = (language - 0x8000) as usize;
            if k >= lang_tags.len() || lang_tags[k] == 0 {
                continue;
            }
        }
        if !ids.is_empty() && !ids.contains(&name_id) {
            continue;
        }
        out.push(NameRecord {
            platform,
            encoding,
            language,
            name_id,
            bytes: d[off..off + len as usize].to_vec(),
        });
    }
    out
}

/// The records of face `index` of the font file `data` (see
/// [`read_name_table`]).
#[must_use]
pub fn read_names(data: &[u8], index: u32, ids: &[u16]) -> Vec<NameRecord> {
    let decoded;
    let data = if crate::woff::is_woff(data) {
        match crate::woff::decode(data) {
            Some(d) => {
                decoded = d;
                &decoded[..]
            }
            None => return Vec::new(),
        }
    } else {
        data
    };
    let Some(off) = sfnt_offset(data, index & 0xFFFF) else {
        return Vec::new();
    };
    let Some(dir) = table_directory(data, off) else {
        return Vec::new();
    };
    let Some(&(_, o, l)) = dir.iter().find(|e| e.0 == tag(b"name")) else {
        return Vec::new();
    };
    read_name_table(&data[o as usize..(o + l) as usize], ids)
}

/// FreeType's `sfnt_is_postscript`: the ASCII characters a PostScript name
/// may hold.
fn is_postscript(c: u8) -> bool {
    const MAP: [u8; 16] = [
        0x00, 0x00, 0x00, 0x00, 0xDE, 0x7C, 0xFF, 0xAF, 0xFF, 0xFF, 0xFF, 0xD7, 0xFF, 0xFF, 0xFF,
        0x57,
    ];
    c < 0x80 && MAP[(c >> 3) as usize] & (1 << (c & 7)) != 0
}

/// `FT_Get_Postscript_Name` for an sfnt face (`sfnt_get_ps_name`): name 6,
/// a Windows record (3,0 or 3,1; the US English one if any, else the
/// first) before an Apple Roman one, keeping only PostScript characters.
#[must_use]
pub fn postscript_name(records: &[NameRecord]) -> Option<String> {
    let mut win = None;
    let mut apple = None;
    for (n, r) in records.iter().enumerate() {
        if r.name_id != POSTSCRIPT_NAME || r.bytes.is_empty() {
            continue;
        }
        if r.platform == 3
            && (r.encoding == 1 || r.encoding == 0)
            && (r.language == 0x409 || win.is_none())
        {
            win = Some(n);
        }
        if r.platform == 1 && r.encoding == 0 && (r.language == 0 || apple.is_none()) {
            apple = Some(n);
        }
    }
    let mut result = None;
    if let Some(w) = win {
        let s: String = records[w]
            .bytes
            .chunks_exact(2)
            .filter(|p| p[0] == 0 && is_postscript(p[1]))
            .map(|p| char::from(p[1]))
            .collect();
        if !s.is_empty() {
            result = Some(s);
        }
    }
    if result.is_none()
        && let Some(a) = apple
    {
        let s: String = records[a]
            .bytes
            .iter()
            .filter(|&&c| is_postscript(c))
            .map(|&c| char::from(c))
            .collect();
        if !s.is_empty() {
            result = Some(s);
        }
    }
    result
}

/// ICU's `macintosh` converter (Mac OS Roman, with the euro at 0xDB and
/// the Apple logo at 0xF0), bytes 0x80–0xFF.
const MAC_ROMAN: [u16; 128] = [
    0x00C4, 0x00C5, 0x00C7, 0x00C9, 0x00D1, 0x00D6, 0x00DC, 0x00E1, 0x00E0, 0x00E2, 0x00E4, 0x00E3,
    0x00E5, 0x00E7, 0x00E9, 0x00E8, 0x00EA, 0x00EB, 0x00ED, 0x00EC, 0x00EE, 0x00EF, 0x00F1, 0x00F3,
    0x00F2, 0x00F4, 0x00F6, 0x00F5, 0x00FA, 0x00F9, 0x00FB, 0x00FC, 0x2020, 0x00B0, 0x00A2, 0x00A3,
    0x00A7, 0x2022, 0x00B6, 0x00DF, 0x00AE, 0x00A9, 0x2122, 0x00B4, 0x00A8, 0x2260, 0x00C6, 0x00D8,
    0x221E, 0x00B1, 0x2264, 0x2265, 0x00A5, 0x00B5, 0x2202, 0x2211, 0x220F, 0x03C0, 0x222B, 0x00AA,
    0x00BA, 0x03A9, 0x00E6, 0x00F8, 0x00BF, 0x00A1, 0x00AC, 0x221A, 0x0192, 0x2248, 0x2206, 0x00AB,
    0x00BB, 0x2026, 0x00A0, 0x00C0, 0x00C3, 0x00D5, 0x0152, 0x0153, 0x2013, 0x2014, 0x201C, 0x201D,
    0x2018, 0x2019, 0x00F7, 0x25CA, 0x00FF, 0x0178, 0x2044, 0x20AC, 0x2039, 0x203A, 0xFB01, 0xFB02,
    0x2021, 0x00B7, 0x201A, 0x201E, 0x2030, 0x00C2, 0x00CA, 0x00C1, 0x00CB, 0x00C8, 0x00CD, 0x00CE,
    0x00CF, 0x00CC, 0x00D3, 0x00D4, 0xF8FF, 0x00D2, 0x00DA, 0x00DB, 0x00D9, 0x0131, 0x02C6, 0x02DC,
    0x00AF, 0x02D8, 0x02D9, 0x02DA, 0x00B8, 0x02DD, 0x02DB, 0x02C7,
];

/// Mac OS Roman bytes as a string (ICU's `macintosh`).
#[must_use]
pub fn decode_mac_roman(b: &[u8]) -> String {
    b.iter()
        .map(|&c| {
            if c < 0x80 {
                char::from(c)
            } else {
                char::from_u32(u32::from(MAC_ROMAN[(c - 0x80) as usize])).unwrap_or('\u{FFFD}')
            }
        })
        .collect()
}

/// UTF-16BE bytes as a string (ICU's `UTF16BE`: an unpaired surrogate or
/// an odd last byte is U+FFFD).
#[must_use]
pub fn decode_utf16be(b: &[u8]) -> String {
    let units: Vec<u16> = b
        .chunks_exact(2)
        .map(|p| u16::from_be_bytes([p[0], p[1]]))
        .collect();
    let mut s: String = char::decode_utf16(units)
        .map(|r| r.unwrap_or('\u{FFFD}'))
        .collect();
    if b.len() % 2 == 1 {
        s.push('\u{FFFD}');
    }
    s
}

/// XeTeXFontMgr_FC's `readNames` decoding of a record: Mac Roman for
/// (1, 0, language 0) — a "preferred" name, put first — and UTF-16BE for
/// Unicode and Microsoft platforms; `None` for other records.
#[must_use]
pub fn xetex_decode(r: &NameRecord) -> Option<(String, bool)> {
    if r.platform == 1 && r.encoding == 0 && r.language == 0 {
        Some((decode_mac_roman(&r.bytes), true))
    } else if r.platform == 0 || r.platform == 3 {
        Some((decode_utf16be(&r.bytes), false))
    } else {
        None
    }
}
