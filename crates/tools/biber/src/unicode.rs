//! The Unicode Perl gives biber: general categories and the binary
//! properties its regexes test (`\p{L}`, `\p{Dash}`, `\w`, `\s`, ...),
//! extended grapheme clusters (`\X`, Unicode::GCString), normalisation
//! (Unicode::Normalize) and case mapping (`lc`, `uc`, `fc`).
//!
//! The tables in `unidata.rs` are generated from the Perl that runs biber
//! (scripts/biber-tables.pl).

use alloc::string::String;
use alloc::vec::Vec;
use unicode_normalization::UnicodeNormalization;

use crate::unidata;

fn in_ranges(t: &[(u32, u32)], c: char) -> bool {
    let c = c as u32;
    t.binary_search_by(|&(s, e)| {
        if e < c {
            core::cmp::Ordering::Less
        } else if s > c {
            core::cmp::Ordering::Greater
        } else {
            core::cmp::Ordering::Equal
        }
    })
    .is_ok()
}

fn in_valued(t: &[(u32, u32, u8)], c: char) -> Option<u8> {
    let c = c as u32;
    t.binary_search_by(|&(s, e, _)| {
        if e < c {
            core::cmp::Ordering::Less
        } else if s > c {
            core::cmp::Ordering::Greater
        } else {
            core::cmp::Ordering::Equal
        }
    })
    .ok()
    .map(|i| t[i].2)
}

/// The two-letter general category (`Lu`, `Mn`, ..., `Cn`).
pub fn gc(c: char) -> &'static str {
    match in_valued(unidata::GC, c) {
        Some(i) => unidata::GC_NAMES[i as usize],
        None => "Cn",
    }
}

/// `\p{X}` for a general category name or major class (`L`, `Lu`, `P`,
/// ...), or one of the binary properties biber's patterns use. `None`
/// for a property this port does not know.
pub fn has_property(c: char, name: &str) -> Option<bool> {
    let g = gc(c);
    let n = name.trim();
    let n = n
        .strip_prefix("Gc=")
        .or_else(|| n.strip_prefix("gc="))
        .or_else(|| n.strip_prefix("General_Category="))
        .unwrap_or(n);
    Some(match n {
        "L" | "Letter" => g.starts_with('L'),
        "LC" | "L&" | "Cased_Letter" => matches!(g, "Lu" | "Ll" | "Lt"),
        "M" | "Mark" => g.starts_with('M'),
        "N" | "Number" => g.starts_with('N'),
        "P" | "Punct" | "Punctuation" => g.starts_with('P'),
        "S" | "Symbol" => g.starts_with('S'),
        "Z" | "Separator" => g.starts_with('Z'),
        "C" | "Other" => g.starts_with('C'),
        "Dash" => in_ranges(unidata::DASH, c),
        "Dia" | "Diacritic" => in_ranges(unidata::DIACRITIC, c),
        "Word" | "XPosixWord" => in_ranges(unidata::WORD, c),
        "Space" | "XPosixSpace" | "White_Space" | "SpacePerl" | "XPerlSpace" => {
            in_ranges(unidata::SPACE, c)
        }
        "Blank" | "XPosixBlank" | "HorizSpace" => in_ranges(unidata::HSPACE, c),
        "Alnum" | "XPosixAlnum" => in_ranges(unidata::ALNUM, c),
        "Digit" | "XPosixDigit" => g == "Nd",
        "ASCII" => (c as u32) < 0x80,
        "ASCII_Hex_Digit" | "AHex" => c.is_ascii_hexdigit(),
        "Alpha" | "Alphabetic" | "XPosixAlpha" => in_ranges(unidata::ALNUM, c) && g != "Nd",
        "Lower" | "Lowercase" | "XPosixLower" => c.is_lowercase(),
        "Upper" | "Uppercase" | "XPosixUpper" => c.is_uppercase(),
        "Any" => true,
        _ => {
            if unidata::GC_NAMES.contains(&n) {
                g == n
            } else {
                return None;
            }
        }
    })
}

pub fn is_l(c: char) -> bool {
    gc(c).starts_with('L')
}
pub fn is_m(c: char) -> bool {
    gc(c).starts_with('M')
}
pub fn is_dash(c: char) -> bool {
    in_ranges(unidata::DASH, c)
}
pub fn is_dia(c: char) -> bool {
    in_ranges(unidata::DIACRITIC, c)
}
/// Perl's `\w`.
pub fn is_word(c: char) -> bool {
    in_ranges(unidata::WORD, c)
}
/// Perl's `\s`.
pub fn is_space(c: char) -> bool {
    in_ranges(unidata::SPACE, c)
}
/// Perl's `\h`.
pub fn is_hspace(c: char) -> bool {
    in_ranges(unidata::HSPACE, c)
}
/// Perl's `\d`.
pub fn is_digit(c: char) -> bool {
    gc(c) == "Nd"
}

/// btparse's `isulower()`: is the first UTF-8 character of a token one of
/// Unicode 6.2's lowercase letters.
pub fn bt_isulower(token: &str) -> bool {
    token
        .chars()
        .next()
        .is_some_and(|c| in_ranges(unidata::ISULOWER, c))
}

/// The decimal digit value of a `\d` character.
pub fn digit_value(c: char) -> Option<u32> {
    if c.is_ascii_digit() {
        return Some(c as u32 - '0' as u32);
    }
    if !is_digit(c) {
        return None;
    }
    // Nd characters come in runs of ten, from zero.
    let cp = c as u32;
    let mut z = cp;
    while z > 0 && is_digit(char::from_u32(z - 1).unwrap_or('\0')) && cp - (z - 1) < 10 {
        z -= 1;
    }
    Some((cp - z) % 10)
}

/// Unicode::UCD's `num()`: the value of a string of digits all of one
/// script, else `None`.
pub fn num(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut v: i64 = 0;
    let mut zero: Option<u32> = None;
    for c in s.chars() {
        let d = digit_value(c)?;
        let z = c as u32 - d;
        if *zero.get_or_insert(z) != z {
            return None;
        }
        v = v.checked_mul(10)?.checked_add(i64::from(d))?;
    }
    Some(v)
}

pub fn nfd(s: &str) -> String {
    s.nfd().collect()
}
pub fn nfc(s: &str) -> String {
    s.nfc().collect()
}
pub fn nfkd(s: &str) -> String {
    s.nfkd().collect()
}
pub fn ccc(c: char) -> u8 {
    unicode_normalization::char::canonical_combining_class(c)
}

/// Perl's `lc`.
pub fn lc(s: &str) -> String {
    s.to_lowercase()
}
/// Perl's `uc`.
pub fn uc(s: &str) -> String {
    s.to_uppercase()
}
/// Perl's `fc` (full case folding).
pub fn fc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'ß' | 'ẞ' => o.push_str("ss"),
            'ŉ' => o.push_str("ʼn"),
            'ſ' => o.push('s'),
            'ς' => o.push('σ'),
            'ﬀ' => o.push_str("ff"),
            'ﬁ' => o.push_str("fi"),
            'ﬂ' => o.push_str("fl"),
            'ﬃ' => o.push_str("ffi"),
            'ﬄ' => o.push_str("ffl"),
            'ﬅ' | 'ﬆ' => o.push_str("st"),
            _ => o.extend(c.to_lowercase()),
        }
    }
    o
}

// ---- grapheme clusters (UAX #29, as Perl's \X) ----

const GCB_OTHER: u8 = 0;
const GCB_CR: u8 = 1;
const GCB_LF: u8 = 2;
const GCB_CONTROL: u8 = 3;
const GCB_EXTEND: u8 = 4;
const GCB_ZWJ: u8 = 5;
const GCB_RI: u8 = 6;
const GCB_PREPEND: u8 = 7;
const GCB_SPACINGMARK: u8 = 8;
const GCB_L: u8 = 9;
const GCB_V: u8 = 10;
const GCB_T: u8 = 11;
const GCB_LV: u8 = 12;
const GCB_LVT: u8 = 13;

fn gcb(c: char) -> u8 {
    in_valued(unidata::GCB, c).unwrap_or(GCB_OTHER)
}

/// The byte offsets where extended grapheme clusters end, in order.
pub fn grapheme_ends(s: &str) -> Vec<usize> {
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let mut ends = Vec::new();
    if chars.is_empty() {
        return ends;
    }
    let mut ri_count = 0usize; // regional indicators in the current run
    let mut ext_pict_zwj; // ExtPict Extend* ZWJ just seen
    let mut in_pict = false; // ExtPict Extend* run
    let mut incb_cons = false; // InCB Consonant [Extend|Linker]* seen
    let mut incb_link = false; // ... with a Linker
    for i in 0..chars.len() {
        let c = chars[i].1;
        let b = gcb(c);
        if i + 1 == chars.len() {
            ends.push(s.len());
            break;
        }
        let n = chars[i + 1].1;
        let nb = gcb(n);
        // state updates for c
        if b == GCB_RI {
            ri_count += 1;
        } else {
            ri_count = 0;
        }
        let pict = in_ranges(unidata::EXTPICT, c);
        if pict {
            in_pict = true;
            ext_pict_zwj = false;
        } else if b == GCB_EXTEND && in_pict {
            ext_pict_zwj = false;
        } else if b == GCB_ZWJ && in_pict {
            ext_pict_zwj = true;
            in_pict = false;
        } else {
            in_pict = false;
            ext_pict_zwj = false;
        }
        if in_ranges(unidata::INCB_CONSONANT, c) {
            incb_cons = true;
            incb_link = false;
        } else if incb_cons && in_ranges(unidata::INCB_LINKER, c) {
            incb_link = true;
        } else if incb_cons && (in_ranges(unidata::INCB_EXTEND, c) || b == GCB_ZWJ) {
        } else {
            incb_cons = false;
            incb_link = false;
        }
        let brk = if b == GCB_CR && nb == GCB_LF {
            false // GB3
        } else if b == GCB_CR || b == GCB_LF || b == GCB_CONTROL {
            true // GB4
        } else if nb == GCB_CR || nb == GCB_LF || nb == GCB_CONTROL {
            true // GB5
        } else if b == GCB_L && matches!(nb, GCB_L | GCB_V | GCB_LV | GCB_LVT) {
            false // GB6
        } else if matches!(b, GCB_LV | GCB_V) && matches!(nb, GCB_V | GCB_T) {
            false // GB7
        } else if matches!(b, GCB_LVT | GCB_T) && nb == GCB_T {
            false // GB8
        } else if nb == GCB_EXTEND || nb == GCB_ZWJ {
            false // GB9
        } else if nb == GCB_SPACINGMARK {
            false // GB9a
        } else if b == GCB_PREPEND {
            false // GB9b
        } else if incb_link && in_ranges(unidata::INCB_CONSONANT, n) {
            false // GB9c
        } else if ext_pict_zwj && in_ranges(unidata::EXTPICT, n) {
            false // GB11
        } else {
            !(b == GCB_RI && nb == GCB_RI && ri_count % 2 == 1) // GB12/13
        };
        if brk {
            ends.push(chars[i + 1].0);
        }
    }
    ends
}

/// The extended grapheme clusters of a string.
pub fn graphemes(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    for e in grapheme_ends(s) {
        out.push(&s[start..e]);
        start = e;
    }
    out
}

/// Unicode::GCString's `length`.
pub fn gc_len(s: &str) -> usize {
    grapheme_ends(s).len()
}

/// Unicode::GCString's `substr($offset, $length)`, with Perl's negative
/// offsets counting from the end.
pub fn gc_substr(s: &str, offset: i64, len: Option<i64>) -> String {
    let g = graphemes(s);
    let n = g.len() as i64;
    let mut start = if offset < 0 { n + offset } else { offset };
    if start < 0 {
        start = 0;
    }
    if start > n {
        return String::new();
    }
    let end = match len {
        None => n,
        Some(l) if l < 0 => (n + l).max(start),
        Some(l) => (start + l).min(n),
    };
    let mut o = String::new();
    for x in &g[start as usize..end.max(start) as usize] {
        o.push_str(x);
    }
    o
}
