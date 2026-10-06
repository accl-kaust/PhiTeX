//! t1_load.c, t1_load.h: Type 1 (PFB) fonts read into a [`CffFont`].
//!
//! `char **enc_vec` (256 glyph names, filled from the font's Encoding;
//! NULL when not wanted) is `Option<&mut [Option<Vec<u8>>]>` of length
//! 256. Parsers `(unsigned char **start, unsigned char *end)` are
//! `(s: &[u8], p: &mut usize)`. Charstrings go through t1_char's state,
//! so the functions reaching it are methods of [`Dpx`].

use crate::cff::*;
use crate::cff_dict::cff_new_dict;
use crate::prelude::*;
use crate::pst::{PstObj, pst_get_token};

pub const T1_EEKEY: u16 = 55665;
pub const T1_CHARKEY: u16 = 4330;
pub const CFF_GLYPH_MAX: i32 = CFF_SID_MAX;
pub const MAX_ARGS: usize = 127;
pub const TYPE1_NAME_LEN_MAX: usize = 127;
pub const PFB_SEG_TYPE_ASCII: i32 = 1;
pub const PFB_SEG_TYPE_BINARY: i32 = 2;

/// `StandardEncoding` (t1_load.c).
pub static STANDARD_ENCODING: [&[u8]; 256] = [
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b"space",
    b"exclam",
    b"quotedbl",
    b"numbersign",
    b"dollar",
    b"percent",
    b"ampersand",
    b"quoteright",
    b"parenleft",
    b"parenright",
    b"asterisk",
    b"plus",
    b"comma",
    b"hyphen",
    b"period",
    b"slash",
    b"zero",
    b"one",
    b"two",
    b"three",
    b"four",
    b"five",
    b"six",
    b"seven",
    b"eight",
    b"nine",
    b"colon",
    b"semicolon",
    b"less",
    b"equal",
    b"greater",
    b"question",
    b"at",
    b"A",
    b"B",
    b"C",
    b"D",
    b"E",
    b"F",
    b"G",
    b"H",
    b"I",
    b"J",
    b"K",
    b"L",
    b"M",
    b"N",
    b"O",
    b"P",
    b"Q",
    b"R",
    b"S",
    b"T",
    b"U",
    b"V",
    b"W",
    b"X",
    b"Y",
    b"Z",
    b"bracketleft",
    b"backslash",
    b"bracketright",
    b"asciicircum",
    b"underscore",
    b"quoteleft",
    b"a",
    b"b",
    b"c",
    b"d",
    b"e",
    b"f",
    b"g",
    b"h",
    b"i",
    b"j",
    b"k",
    b"l",
    b"m",
    b"n",
    b"o",
    b"p",
    b"q",
    b"r",
    b"s",
    b"t",
    b"u",
    b"v",
    b"w",
    b"x",
    b"y",
    b"z",
    b"braceleft",
    b"bar",
    b"braceright",
    b"asciitilde",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b"exclamdown",
    b"cent",
    b"sterling",
    b"fraction",
    b"yen",
    b"florin",
    b"section",
    b"currency",
    b"quotesingle",
    b"quotedblleft",
    b"guillemotleft",
    b"guilsinglleft",
    b"guilsinglright",
    b"fi",
    b"fl",
    b".notdef",
    b"endash",
    b"dagger",
    b"daggerdbl",
    b"periodcentered",
    b".notdef",
    b"paragraph",
    b"bullet",
    b"quotesinglbase",
    b"quotedblbase",
    b"quotedblright",
    b"guillemotright",
    b"ellipsis",
    b"perthousand",
    b".notdef",
    b"questiondown",
    b".notdef",
    b"grave",
    b"acute",
    b"circumflex",
    b"tilde",
    b"macron",
    b"breve",
    b"dotaccent",
    b"dieresis",
    b".notdef",
    b"ring",
    b"cedilla",
    b".notdef",
    b"hungarumlaut",
    b"ogonek",
    b"caron",
    b"emdash",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b"AE",
    b".notdef",
    b"ordfeminine",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b"Lslash",
    b"Oslash",
    b"OE",
    b"ordmasculine",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b"ae",
    b".notdef",
    b".notdef",
    b".notdef",
    b"dotlessi",
    b".notdef",
    b".notdef",
    b"lslash",
    b"oslash",
    b"oe",
    b"germandbls",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
];

/// `ISOLatin1Encoding` (t1_load.c).
pub static ISO_LATIN1_ENCODING: [&[u8]; 256] = [
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b"space",
    b"exclam",
    b"quotedbl",
    b"numbersign",
    b"dollar",
    b"percent",
    b"ampersand",
    b"quotesingle",
    b"parenleft",
    b"parenright",
    b"asterisk",
    b"plus",
    b"comma",
    b"hyphen",
    b"period",
    b"slash",
    b"zero",
    b"one",
    b"two",
    b"three",
    b"four",
    b"five",
    b"six",
    b"seven",
    b"eight",
    b"nine",
    b"colon",
    b"semicolon",
    b"less",
    b"equal",
    b"greater",
    b"question",
    b"at",
    b"A",
    b"B",
    b"C",
    b"D",
    b"E",
    b"F",
    b"G",
    b"H",
    b"I",
    b"J",
    b"K",
    b"L",
    b"M",
    b"N",
    b"O",
    b"P",
    b"Q",
    b"R",
    b"S",
    b"T",
    b"U",
    b"V",
    b"W",
    b"X",
    b"Y",
    b"Z",
    b"bracketleft",
    b"backslash",
    b"bracketright",
    b"asciicircum",
    b"underscore",
    b"grave",
    b"a",
    b"b",
    b"c",
    b"d",
    b"e",
    b"f",
    b"g",
    b"h",
    b"i",
    b"j",
    b"k",
    b"l",
    b"m",
    b"n",
    b"o",
    b"p",
    b"q",
    b"r",
    b"s",
    b"t",
    b"u",
    b"v",
    b"w",
    b"x",
    b"y",
    b"z",
    b"braceleft",
    b"bar",
    b"braceright",
    b"asciitilde",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b".notdef",
    b"dotlessi",
    b"quoteleft",
    b"quoteright",
    b"circumflex",
    b"tilde",
    b"macron",
    b"breve",
    b"dotaccent",
    b"dieresis",
    b".notdef",
    b"ring",
    b"cedilla",
    b".notdef",
    b"hungarumlaut",
    b"ogonek",
    b"caron",
    b"space",
    b"exclamdown",
    b"cent",
    b"sterling",
    b"currency",
    b"yen",
    b"brokenbar",
    b"section",
    b"dieresis",
    b"copyright",
    b"ordfeminine",
    b"guillemotleft",
    b"logicalnot",
    b"hyphen",
    b"registered",
    b"macron",
    b"degree",
    b"plusminus",
    b"twosuperior",
    b"threesuperior",
    b"acute",
    b"mu",
    b"paragraph",
    b"periodcentered",
    b"cedilla",
    b"onesuperior",
    b"ordmasculine",
    b"guillemotright",
    b"onequarter",
    b"onehalf",
    b"threequarters",
    b"questiondown",
    b"Agrave",
    b"Aacute",
    b"Acircumflex",
    b"Atilde",
    b"Adieresis",
    b"Aring",
    b"AE",
    b"Ccedilla",
    b"Egrave",
    b"Eacute",
    b"Ecircumflex",
    b"Edieresis",
    b"Igrave",
    b"Iacute",
    b"Icircumflex",
    b"Idieresis",
    b"Eth",
    b"Ntilde",
    b"Ograve",
    b"Oacute",
    b"Ocircumflex",
    b"Otilde",
    b"Odieresis",
    b"multiply",
    b"Oslash",
    b"Ugrave",
    b"Uacute",
    b"Ucircumflex",
    b"Udieresis",
    b"Yacute",
    b"Thorn",
    b"germandbls",
    b"agrave",
    b"aacute",
    b"acircumflex",
    b"atilde",
    b"adieresis",
    b"aring",
    b"ae",
    b"ccedilla",
    b"egrave",
    b"eacute",
    b"ecircumflex",
    b"edieresis",
    b"igrave",
    b"iacute",
    b"icircumflex",
    b"idieresis",
    b"eth",
    b"ntilde",
    b"ograve",
    b"oacute",
    b"ocircumflex",
    b"otilde",
    b"odieresis",
    b"divide",
    b"oslash",
    b"ugrave",
    b"uacute",
    b"ucircumflex",
    b"udieresis",
    b"yacute",
    b"thorn",
    b"ydieresis",
];

/// `CS_STR_LEN_MAX` (t1_load.c's, as cff_limits.h's).
const T1_CS_STR_LEN_MAX: i32 = 65536;

/// `t1_decrypt` (static): `src[..len]` decrypted, its first `skip`
/// bytes dropped, into `dst[..len - skip]`.
fn t1_decrypt(key: u16, dst: &mut [u8], src: &[u8], skip: i32, len: i32) {
    let mut key = key;
    let len = len - skip;
    let mut s = 0usize;
    for _ in 0..skip.max(0) {
        key = (key.wrapping_add(u16::from(src[s])))
            .wrapping_mul(52845)
            .wrapping_add(22719);
        s += 1;
    }
    // (C loops on a negative length.)
    for d in 0..len.max(0) as usize {
        let c = src[s];
        s += 1;
        dst[d] = c ^ (key >> 8) as u8;
        key = (key.wrapping_add(u16::from(c)))
            .wrapping_mul(52845)
            .wrapping_add(22719);
    }
}

/// `t1_decrypt(key, buf, buf, 0, len)`: in place.
fn t1_decrypt_in_place(key: u16, buf: &mut [u8]) {
    let mut key = key;
    for b in buf.iter_mut() {
        let c = *b;
        *b = c ^ (key >> 8) as u8;
        key = (key.wrapping_add(u16::from(c)))
            .wrapping_mul(52845)
            .wrapping_add(22719);
    }
}

/// `MATCH_NAME` (C dereferences a NULL token; none here does not match).
fn match_name(t: Option<&PstObj>, n: &[u8]) -> Result<bool> {
    Ok(match t {
        Some(t) => t.is_name() && t.pst_length_of()? as usize == n.len() && t.pst_data_ptr()? == n,
        None => false,
    })
}

/// `MATCH_OP` (C dereferences a NULL token; none here does not match).
fn match_op(t: Option<&PstObj>, n: &[u8]) -> Result<bool> {
    Ok(match t {
        Some(t) => {
            t.is_unknown() && t.pst_length_of()? as usize == n.len() && t.pst_data_ptr()? == n
        }
        None => false,
    })
}

/// `PST_INTEGERTYPE(tok)` (none: C dereferences NULL).
fn is_integer(t: Option<&PstObj>) -> bool {
    t.is_some_and(PstObj::is_integer)
}

/// `PST_NAMETYPE(tok)` (none: C dereferences NULL).
fn is_name(t: Option<&PstObj>) -> bool {
    t.is_some_and(PstObj::is_name)
}

/// `pst_getIV(tok)` where C has checked `PST_INTEGERTYPE(tok)` first
/// (0 otherwise: every use is behind that check).
fn iv(t: Option<&PstObj>) -> i32 {
    match t {
        Some(PstObj::Integer(v)) => *v,
        _ => 0,
    }
}

/// A string as C sees it (up to its first NUL).
fn cstr(mut v: Vec<u8>) -> Vec<u8> {
    if let Some(n) = v.iter().position(|&c| c == 0) {
        v.truncate(n);
    }
    v
}

/// `get_next_key` (static).
fn get_next_key(s: &[u8], p: &mut usize) -> Result<Option<Vec<u8>>> {
    let mut key = None;

    while *p < s.len() {
        let Some(tok) = pst_get_token(s, p)? else {
            break;
        };
        if tok.is_name() {
            key = tok.pst_getSV()?;
            break;
        }
    }

    Ok(key)
}

/// `seek_operator` (static): 0 found, -1 not.
fn seek_operator(s: &[u8], p: &mut usize, op: &[u8]) -> Result<i32> {
    let mut tok: Option<PstObj> = None;

    while *p < s.len() {
        tok = pst_get_token(s, p)?;
        if tok.is_none() {
            break;
        }
        if match_op(tok.as_ref(), op)? {
            break;
        }
        tok = None;
    }

    if tok.is_none() {
        return Ok(-1);
    }

    Ok(0)
}

/// `parse_svalue` (static): status and the value.
fn parse_svalue(s: &[u8], p: &mut usize) -> Result<(i32, Option<Vec<u8>>)> {
    let Some(tok) = pst_get_token(s, p)? else {
        return Ok((-1, None));
    };
    if tok.is_name() || tok.is_string() {
        Ok((1, tok.pst_getSV()?.map(cstr)))
    } else {
        Ok((-1, None))
    }
}

/// `parse_bvalue` (static): status and the value.
fn parse_bvalue(s: &[u8], p: &mut usize) -> Result<(i32, f64)> {
    let Some(tok) = pst_get_token(s, p)? else {
        return Ok((-1, 0.0));
    };
    if tok.is_boolean() {
        Ok((1, f64::from(tok.pst_getIV()?)))
    } else {
        Ok((-1, 0.0))
    }
}

/// `parse_nvalue` (static): the count read (or < 0) into `value`
/// (at most `max`).
fn parse_nvalue(s: &[u8], p: &mut usize, value: &mut [f64], max: i32) -> Result<i32> {
    let mut argn = 0;

    let mut tok = pst_get_token(s, p)?;
    let Some(t) = &tok else {
        return Ok(-1);
    };
    // All array elements must be numeric token. (ATM compatible)
    if t.is_number() && max > 0 {
        value[0] = t.pst_getRV()?;
        argn = 1;
    } else if t.is_mark() {
        // It does not distinguish '[' and '{'...
        tok = None;
        while *p < s.len() {
            tok = pst_get_token(s, p)?;
            match &tok {
                Some(t) if t.is_number() && argn < max => {
                    value[argn as usize] = t.pst_getRV()?;
                    argn += 1;
                    tok = None;
                }
                _ => break,
            }
        }
        if tok.is_none() {
            return Ok(-1);
        }
        if !match_op(tok.as_ref(), b"]")? && !match_op(tok.as_ref(), b"}")? {
            argn = -1;
        }
    }

    Ok(argn)
}

/// `xstrdup(enc_vec[n])` (kpathsea's crashes on NULL; none stays none).
fn dup_name(v: &Option<Vec<u8>>) -> Option<Vec<u8>> {
    v.clone()
}

/// `try_put_or_putinterval` (static): "dup num num getinterval num exch
/// putinterval" or "dup num exch num get put".
fn try_put_or_putinterval(enc_vec: &mut [Option<Vec<u8>>], s: &[u8], p: &mut usize) -> Result<i32> {
    let tok = pst_get_token(s, p)?;
    let num1 = iv(tok.as_ref());
    if !is_integer(tok.as_ref()) || num1 > 255 || num1 < 0 {
        return Ok(-1);
    }

    let tok = pst_get_token(s, p)?;
    if tok.is_none() {
        return Ok(-1);
    } else if match_op(tok.as_ref(), b"exch")? {
        // dup num exch num get put
        let tok = pst_get_token(s, p)?;
        let num2 = iv(tok.as_ref());
        if !is_integer(tok.as_ref()) || num2 > 255 || num2 < 0 {
            return Ok(-1);
        }

        let tok = pst_get_token(s, p)?;
        if !match_op(tok.as_ref(), b"get")? {
            return Ok(-1);
        }

        let tok = pst_get_token(s, p)?;
        if !match_op(tok.as_ref(), b"put")? {
            return Ok(-1);
        }

        enc_vec[num1 as usize] = dup_name(&enc_vec[num2 as usize]);
    } else if is_integer(tok.as_ref()) && {
        let num2 = iv(tok.as_ref());
        num2 + num1 <= 255 && num2 >= 0
    } {
        let num2 = iv(tok.as_ref());

        let tok = pst_get_token(s, p)?;
        if !match_op(tok.as_ref(), b"getinterval")? {
            return Ok(-1);
        }

        let tok = pst_get_token(s, p)?;
        let num3 = iv(tok.as_ref());
        if !is_integer(tok.as_ref()) || num3 + num2 > 255 || num3 < 0 {
            return Ok(-1);
        }

        let tok = pst_get_token(s, p)?;
        if !match_op(tok.as_ref(), b"exch")? {
            return Ok(-1);
        }

        let tok = pst_get_token(s, p)?;
        if !match_op(tok.as_ref(), b"putinterval")? {
            return Ok(-1);
        }

        for i in 0..num2 {
            if enc_vec[(num1 + i) as usize].is_some() {
                // num1 + i < 256 here; num3 + i < 256 here
                // (C frees the old name first, even when it is the one
                // copied: the copy is taken before here.)
                let v = dup_name(&enc_vec[(num1 + i) as usize]);
                enc_vec[(num3 + i) as usize] = v;
            }
        }
    } else {
        return Ok(-1);
    }

    Ok(0)
}

/// `parse_encoding` (static).
fn parse_encoding(
    mut enc_vec: Option<&mut [Option<Vec<u8>>]>,
    s: &[u8],
    p: &mut usize,
) -> Result<i32> {
    //  StandardEncoding def
    // or
    //  ISOLatin1Encoding def
    // or
    //  0 1 255 {1 index exch /.notdef put } for
    //  dup int name put
    //  ...
    //  [readonly] def
    let tok = pst_get_token(s, p)?;
    if match_op(tok.as_ref(), b"StandardEncoding")? {
        if let Some(enc_vec) = enc_vec {
            for code in 0..256 {
                enc_vec[code] = if STANDARD_ENCODING[code] != b".notdef" {
                    Some(STANDARD_ENCODING[code].to_vec())
                } else {
                    None
                };
            }
        }
    } else if match_op(tok.as_ref(), b"ISOLatin1Encoding")? {
        if let Some(enc_vec) = enc_vec {
            for code in 0..256 {
                enc_vec[code] = if ISO_LATIN1_ENCODING[code] != b".notdef" {
                    Some(ISO_LATIN1_ENCODING[code].to_vec())
                } else {
                    None
                };
            }
        }
    } else if match_op(tok.as_ref(), b"ExpertEncoding")? {
        if enc_vec.is_some() {
            warn!("ExpertEncoding not supported.");
            return Ok(-1);
        }
        // Not supported yet.
    } else {
        seek_operator(s, p, b"array")?;
        // Pick all seaquences that matches "dup n /Name put" until
        // occurrence of "def" or "readonly".
        while *p < s.len() {
            let tok = pst_get_token(s, p)?;
            if tok.is_none() {
                break;
            }
            if match_op(tok.as_ref(), b"def")? || match_op(tok.as_ref(), b"readonly")? {
                break;
            } else if !match_op(tok.as_ref(), b"dup")? {
                continue;
            }

            // cmctt10.pfb for examples contains the following PS code
            //     dup num num getinterval num exch putinterval
            //     dup num exch num get put
            let tok = pst_get_token(s, p)?;
            if match_op(tok.as_ref(), b"dup")? {
                // possibly putinterval type
                match enc_vec.as_deref_mut() {
                    None => {
                        warn!(
                            "This kind of type1 fonts are not supported as native fonts.\n                   They are supported if used with tfm fonts.\n"
                        );
                    }
                    Some(enc_vec) => {
                        try_put_or_putinterval(enc_vec, s, p)?;
                    }
                }
                continue;
            }
            let code = iv(tok.as_ref());
            if !is_integer(tok.as_ref()) || code > 255 || code < 0 {
                continue;
            }

            let tok = pst_get_token(s, p)?;
            if !is_name(tok.as_ref()) {
                continue;
            }
            if let Some(enc_vec) = enc_vec.as_deref_mut() {
                enc_vec[code as usize] = tok.as_ref().map(PstObj::pst_getSV).transpose()?.flatten();
            }

            let tok = pst_get_token(s, p)?;
            if !match_op(tok.as_ref(), b"put")? {
                // (C dereferences a NULL enc_vec here.)
                if let Some(enc_vec) = enc_vec.as_deref_mut() {
                    enc_vec[code as usize] = None;
                }
                continue;
            }
        }
    }

    Ok(0)
}

/// `CHECK_ARGN_EQ(n)`, `CHECK_ARGN_GE(n)`: false (C returns -1).
fn check_argn_eq(argn: i32, n: i32) -> bool {
    if argn != n {
        warn!("{} values expected but only {} read.", n, argn);
        return false;
    }
    true
}
fn check_argn_ge(argn: i32, n: i32) -> bool {
    if argn < n {
        warn!("{} values expected but only {} read.", n, argn);
        return false;
    }
    true
}

/// `font->topdict`.
fn topdict(font: &mut CffFont) -> &mut CffDict {
    font.topdict.as_mut().expect("topdict")
}

/// `font->private[0]`.
fn private0(font: &mut CffFont) -> &mut CffDict {
    font.private[0].as_mut().expect("private")
}

/// `parse_part1` (static).
fn parse_part1(
    font: &mut CffFont,
    mut enc_vec: Option<&mut [Option<Vec<u8>>]>,
    s: &[u8],
    p: &mut usize,
) -> Result<i32> {
    let mut argv = [0.0f64; MAX_ARGS];

    // We skip PostScript code inserted before the beginning of font
    // dictionary so that parser will not be confused with it. See
    // LMRoman10-Regular (lmr10.pfb) for example.
    if seek_operator(s, p, b"begin")? < 0 {
        return Ok(-1);
    }

    while *p < s.len() {
        let Some(key) = get_next_key(s, p)? else {
            break;
        };
        let k = key.as_slice();
        if k == b"Encoding" {
            if parse_encoding(enc_vec.as_deref_mut(), s, p)? < 0 {
                return Ok(-1);
            }
        } else if k == b"FontName" {
            let (argn, strval) = parse_svalue(s, p)?;
            if !check_argn_eq(argn, 1) {
                return Ok(-1);
            }
            let mut strval = strval.unwrap_or_default();
            if strval.len() > TYPE1_NAME_LEN_MAX {
                warn!("FontName too long: ({} bytes)", strval.len());
                strval.truncate(TYPE1_NAME_LEN_MAX);
            }
            font.cff_set_name(&strval)?;
        } else if k == b"FontType" {
            let argn = parse_nvalue(s, p, &mut argv, 1)?;
            if !check_argn_eq(argn, 1) {
                return Ok(-1);
            }
            if argv[0] != 1.0 {
                warn!("FontType {} not supported.", argv[0] as i32);
                return Ok(-1);
            }
        } else if k == b"ItalicAngle" || k == b"StrokeWidth" || k == b"PaintType" {
            let argn = parse_nvalue(s, p, &mut argv, 1)?;
            if !check_argn_eq(argn, 1) {
                return Ok(-1);
            }
            if argv[0] != 0.0 {
                topdict(font).cff_dict_add(k, 1)?;
                topdict(font).cff_dict_set(k, 0, argv[0])?;
            }
        } else if k == b"UnderLinePosition" || k == b"UnderLineThickness" {
            let argn = parse_nvalue(s, p, &mut argv, 1)?;
            if !check_argn_eq(argn, 1) {
                return Ok(-1);
            }
            topdict(font).cff_dict_add(k, 1)?;
            topdict(font).cff_dict_set(k, 0, argv[0])?;
        } else if k == b"FontBBox" {
            let mut argn = parse_nvalue(s, p, &mut argv, 4)?;
            if !check_argn_eq(argn, 4) {
                return Ok(-1);
            }
            topdict(font).cff_dict_add(k, 4)?;
            while argn > 0 {
                argn -= 1;
                topdict(font).cff_dict_set(k, argn, argv[argn as usize])?;
            }
        } else if k == b"FontMatrix" {
            let mut argn = parse_nvalue(s, p, &mut argv, 6)?;
            if !check_argn_eq(argn, 6) {
                return Ok(-1);
            }
            if argv[0] != 0.001
                || argv[1] != 0.0
                || argv[2] != 0.0
                || argv[3] != 0.001
                || argv[4] != 0.0
                || argv[5] != 0.0
            {
                topdict(font).cff_dict_add(k, 6)?;
                while argn > 0 {
                    argn -= 1;
                    topdict(font).cff_dict_set(k, argn, argv[argn as usize])?;
                }
            }
        } else if k == b"version"
            || k == b"Notice"
            || k == b"FullName"
            || k == b"FamilyName"
            || k == b"Weight"
            || k == b"Copyright"
        {
            // FontInfo
            let (argn, strval) = parse_svalue(s, p)?;
            if !check_argn_eq(argn, 1) {
                return Ok(-1);
            }
            let strval = strval.unwrap_or_default();
            topdict(font).cff_dict_add(k, 1)?;
            let mut sid = font.cff_get_sid(&strval) as SSid;
            if i32::from(sid) == CFF_STRING_NOTDEF {
                sid = font.cff_add_string(&strval, 0); // FIXME
            }
            // We don't care about duplicate strings here since later a
            // subset font of this font will be generated.
            topdict(font).cff_dict_set(k, 0, f64::from(sid))?;
        } else if k == b"IsFixedPitch" {
            let (argn, v) = parse_bvalue(s, p)?;
            argv[0] = v;
            if !check_argn_eq(argn, 1) {
                return Ok(-1);
            }
            if argv[0] != 0.0 {
                private0(font).cff_dict_add(k, 1)?;
                private0(font).cff_dict_set(k, 0, 1.0)?;
            }
        }
    }

    Ok(0)
}

/// `get_pfb_segment` (static): the segment (C's `*length` = its len),
/// or none.
fn get_pfb_segment(fp: &mut MemFile, expected_type: i32) -> Result<Option<Vec<u8>>> {
    let mut buffer: Vec<u8> = Vec::new();
    loop {
        let ch = fp.getc();
        if ch < 0 {
            break;
        } else if ch != 128 {
            fatal!("Not a pfb file?");
        }
        let ch = fp.getc();
        if ch < 0 || ch != expected_type {
            fp.seek_relative(-2);
            break;
        }
        let mut slen: i32 = 0;
        for i in 0..4 {
            let ch = fp.getc();
            if ch < 0 {
                return Ok(None);
            }
            slen = slen.wrapping_add(ch << (8 * i));
        }
        let start = buffer.len();
        buffer.resize(start + slen.max(0) as usize, 0);
        let mut bytesread = start;
        while slen > 0 {
            let rlen = fp.read_into(&mut buffer[bytesread..bytesread + slen as usize]);
            if rlen == 0 {
                // C's fread loop never ends on a short file.
                return Ok(None);
            }
            slen -= rlen as i32;
            bytesread += rlen;
        }
    }
    if buffer.is_empty() {
        fatal!("PFB segment length zero?");
    }

    Ok(Some(buffer))
}

/// `init_cff_font` (static).
fn init_cff_font(cff: &mut CffFont) {
    cff.stream = None;
    cff.filter = 0;
    cff.fontname = None;
    cff.index = 0;
    cff.flag = FONTTYPE_FONT;

    cff.header.major = 1;
    cff.header.minor = 0;
    cff.header.hdr_size = 4;
    cff.header.offsize = 4;
    cff.name = Some(CffIndex::cff_new_index(1));
    cff.topdict = Some(cff_new_dict());
    cff.string = None;
    cff.gsubr = Some(CffIndex::cff_new_index(0)); // No Global Subr
    cff.encoding = None;
    cff.charsets = None;
    cff.fdselect = None;
    cff.cstrings = None;
    cff.fdarray = Vec::new();
    cff.private = vec![Some(cff_new_dict())];
    cff.subrs = vec![None];

    cff.offset = 0;
    cff.gsubr_offset = 0;
    cff.num_glyphs = 0;
    cff.num_fds = 1;
    cff._string = Some(CffIndex::cff_new_index(0));
}

/// `is_pfb`.
pub fn is_pfb(fp: &mut MemFile) -> i32 {
    let mut sig = [0u8; 15];

    fp.rewind();
    let ch = fp.getc();
    if ch != 128 {
        return 0;
    }
    let ch = fp.getc();
    if ch < 0 || ch > 3 {
        return 0;
    }
    for _ in 0..4 {
        if fp.getc() < 0 {
            return 0;
        }
    }
    for i in 0..14 {
        let ch = fp.getc();
        if ch < 0 {
            return 0;
        }
        sig[i] = ch as u8;
    }
    if &sig[..14] == b"%!PS-AdobeFont" || &sig[..11] == b"%!FontType1" {
        1
    } else if &sig[..4] == b"%!PS" {
        warn!("Ambiguous PostScript resource type.");
        1
    } else {
        warn!("Not a PFB font file?");
        0
    }
}

/// `t1_get_standard_glyph`: StandardEncoding's name of `code`.
pub fn t1_get_standard_glyph(code: i32) -> Option<&'static [u8]> {
    Some(STANDARD_ENCODING[code as usize])
}

/// `t1_get_fontname`: status (0 ok, -1) and the FontName (C's buffer,
/// empty if none was found).
pub fn t1_get_fontname(fp: &mut MemFile) -> Result<(i32, Vec<u8>)> {
    let mut fontname = Vec::new();
    let mut fn_found = false;

    fp.rewind();
    let buffer = match get_pfb_segment(fp, PFB_SEG_TYPE_ASCII)? {
        Some(b) if !b.is_empty() => b,
        _ => fatal!("Reading PFB (ASCII part) file failed."),
    };
    let s = buffer.as_slice();
    let mut p = 0;

    if seek_operator(s, &mut p, b"begin")? < 0 {
        return Ok((-1, fontname));
    }

    while !fn_found && p < s.len() {
        let Some(key) = get_next_key(s, &mut p)? else {
            break;
        };
        if key == b"FontName" {
            let (st, strval) = parse_svalue(s, &mut p)?;
            if st == 1 {
                let mut strval = strval.unwrap_or_default();
                if strval.len() > TYPE1_NAME_LEN_MAX {
                    warn!("FontName too long. ({} bytes)", strval.len());
                    strval.truncate(TYPE1_NAME_LEN_MAX);
                }
                fontname = strval;
                fn_found = true;
            }
        }
    }

    Ok((0, fontname))
}

impl Dpx {
    /// `parse_subrs` (static; prefixed: Dpx methods share a namespace).
    fn t1_load_parse_subrs(
        &mut self,
        font: &mut CffFont,
        s: &[u8],
        p: &mut usize,
        len_iv: i32,
        mode: i32,
    ) -> Result<i32> {
        let tok = pst_get_token(s, p)?;
        if !is_integer(tok.as_ref()) || iv(tok.as_ref()) < 0 {
            warn!("Parsing Subrs failed.");
            return Ok(-1);
        }

        let count = iv(tok.as_ref());

        if count == 0 {
            font.subrs[0] = None;
            return Ok(0);
        }

        let tok = pst_get_token(s, p)?;
        if !match_op(tok.as_ref(), b"array")? {
            return Ok(-1);
        }

        let mut max_size: i32;
        let mut data: Vec<u8>;
        let mut offsets: Vec<i32>;
        let mut lengths: Vec<i32>;
        if mode != 1 {
            max_size = T1_CS_STR_LEN_MAX;
            data = vec![0; max_size as usize];
            offsets = vec![0; count as usize];
            lengths = vec![0; count as usize];
        } else {
            max_size = 0;
            data = Vec::new();
            offsets = Vec::new();
            lengths = Vec::new();
        }

        let mut offset: i32 = 0;
        // dup subr# n-bytes RD n-binary-bytes NP
        let mut i = 0;
        while i < count {
            let tok = pst_get_token(s, p)?;
            if tok.is_none() {
                return Ok(-1);
            } else if match_op(tok.as_ref(), b"ND")?
                || match_op(tok.as_ref(), b"|-")?
                || match_op(tok.as_ref(), b"def")?
            {
                break;
            } else if !match_op(tok.as_ref(), b"dup")? {
                continue;
            }

            // Found "dup"
            let tok = pst_get_token(s, p)?;
            if !is_integer(tok.as_ref()) || iv(tok.as_ref()) < 0 || iv(tok.as_ref()) >= count {
                return Ok(-1);
            }
            let idx = iv(tok.as_ref()) as usize;

            let tok = pst_get_token(s, p)?;
            if !is_integer(tok.as_ref())
                || iv(tok.as_ref()) < 0
                || iv(tok.as_ref()) > T1_CS_STR_LEN_MAX
            {
                return Ok(-1);
            }
            let len = iv(tok.as_ref());

            let tok = pst_get_token(s, p)?;
            if !match_op(tok.as_ref(), b"RD")?
                && !match_op(tok.as_ref(), b"-|")?
                && seek_operator(s, p, b"readstring")? < 0
            {
                return Ok(-1);
            }

            *p += 1;
            if *p + len as usize >= s.len() {
                return Ok(-1);
            }
            if mode != 1 {
                if offset + len >= max_size {
                    max_size += T1_CS_STR_LEN_MAX;
                    data.resize(max_size as usize, 0);
                }
                if len_iv >= 0 {
                    t1_decrypt(
                        T1_CHARKEY,
                        &mut data[offset as usize..],
                        &s[*p..],
                        len_iv,
                        len,
                    );
                    offsets[idx] = offset;
                    lengths[idx] = len - len_iv;
                    offset += lengths[idx];
                } else if len > 0 {
                    offsets[idx] = offset;
                    lengths[idx] = len;
                    data[offset as usize..(offset + len) as usize]
                        .copy_from_slice(&s[*p..*p + len as usize]);
                    offset += len;
                }
            }
            *p += len as usize;
            i += 1;
        }

        if mode != 1 {
            if font.subrs[0].is_none() {
                let mut subrs = CffIndex::cff_new_index(count as Card16);
                subrs.data = vec![0; offset.max(0) as usize];
                let mut offset: i32 = 0;
                for i in 0..count as usize {
                    subrs.offset[i] = (offset + 1) as LOffset;
                    if lengths[i] > 0 {
                        let (o, l) = (offsets[i] as usize, lengths[i] as usize);
                        subrs.data[offset as usize..offset as usize + l]
                            .copy_from_slice(&data[o..o + l]);
                        offset += lengths[i];
                    }
                }
                subrs.offset[count as usize] = (offset + 1) as LOffset;
                font.subrs[0] = Some(subrs);
            } else {
                // Adobe's OPO_____.PFB and OPBO____.PFB have two /Subrs
                // dicts, and also have /CharStrings not followed by dicts.
                // Simply ignores those data. By ChoF on 2009/04/08.
                warn!("Already found /Subrs; ignores the other /Subrs dicts.");
            }
        }

        Ok(0)
    }
    /// `parse_charstrings` (static, prefixed).
    fn t1_load_parse_charstrings(
        &mut self,
        font: &mut CffFont,
        s: &[u8],
        p: &mut usize,
        len_iv: i32,
        mode: i32,
    ) -> Result<i32> {
        // /CharStrings n dict dup begin
        // /GlyphName n-bytes RD -n-binary-bytes- ND
        // ...
        // end
        //  - stack - ... /CharStrings dict
        let tok = pst_get_token(s, p)?;
        if !is_integer(tok.as_ref()) || iv(tok.as_ref()) < 0 || iv(tok.as_ref()) > CFF_GLYPH_MAX {
            let _s = tok.as_ref().map(PstObj::pst_getSV).transpose()?.flatten();
            warn!("Ignores non dict \"/CharStrings ...\"");
            return Ok(0);
        }
        let count = iv(tok.as_ref());

        let mut max_size: i32;
        if mode != 1 {
            let mut charstrings = CffIndex::cff_new_index(count as Card16);
            max_size = T1_CS_STR_LEN_MAX;
            charstrings.data = vec![0; max_size as usize];
            font.cstrings = Some(charstrings);
        } else {
            font.cstrings = None;
            max_size = 0;
        }

        // (C allocates count - 1 entries, as a card16 count.)
        let n = (count - 1) as Card16;
        font.charsets = Some(CffCharsets {
            format: 0,
            num_entries: n,
            glyphs: vec![0; usize::from(n)],
            ..CffCharsets::default()
        });

        let mut offset: i32 = 0;
        let mut have_notdef = false; // .notdef must be at gid = 0 in CFF

        font.is_notdef_notzero = 0;
        seek_operator(s, p, b"begin")?;
        let mut i = 0;
        while i < count {
            // BUG-20061126 (by ChoF):
            // Some fonts (e.g., belleek/blsy.pfb) does not have the correct
            // number of glyphs. Modify the codes even to work with these
            // broken fonts.
            let tok = pst_get_token(s, p)?;
            let glyph_name = tok.as_ref().map(PstObj::pst_getSV).transpose()?.flatten();

            if i == 0 && glyph_name.as_deref().is_some_and(|g| g != b".notdef") {
                font.is_notdef_notzero = 1;
            }

            let gid: i32;
            if is_name(tok.as_ref()) {
                let Some(g) = glyph_name.as_deref() else {
                    return Ok(-1);
                };
                if g == b".notdef" {
                    gid = 0;
                    have_notdef = true;
                } else if have_notdef {
                    gid = i;
                } else if i == count - 1 {
                    warn!("No .notdef glyph???");
                    return Ok(-1);
                } else {
                    gid = i + 1;
                }
            } else if tok.as_ref().is_some_and(PstObj::is_unknown)
                && glyph_name.as_deref() == Some(b"end")
            {
                break;
            } else {
                return Ok(-1);
            }

            if gid > 0 {
                let sid = font.cff_add_string(glyph_name.as_deref().unwrap_or_default(), 0);
                font.charsets.as_mut().unwrap().glyphs[(gid - 1) as usize] = sid;
            }
            // We don't care about duplicate strings here since later a
            // subset font of this font will be generated.

            let tok = pst_get_token(s, p)?;
            if !is_integer(tok.as_ref())
                || iv(tok.as_ref()) < 0
                || iv(tok.as_ref()) > T1_CS_STR_LEN_MAX
            {
                return Ok(-1);
            }
            let len = iv(tok.as_ref());

            let tok = pst_get_token(s, p)?;
            if !match_op(tok.as_ref(), b"RD")?
                && !match_op(tok.as_ref(), b"-|")?
                && seek_operator(s, p, b"readstring")? < 0
            {
                return Ok(-1);
            }

            if *p + len as usize + 1 >= s.len() {
                return Ok(-1);
            }
            if mode != 1 {
                let charstrings = font.cstrings.as_mut().unwrap();
                if offset + len >= max_size {
                    max_size += len.max(T1_CS_STR_LEN_MAX);
                    charstrings.data.resize(max_size as usize, 0);
                }
                if gid == 0 {
                    let shift = if len_iv >= 0 { len - len_iv } else { len };
                    charstrings
                        .data
                        .copy_within(0..offset as usize, shift as usize);
                    for j in 1..=i as usize {
                        charstrings.offset[j] =
                            (charstrings.offset[j] as i32).wrapping_add(shift) as LOffset;
                    }
                }
            }

            *p += 1;
            if mode != 1 {
                let charstrings = font.cstrings.as_mut().unwrap();
                if len_iv >= 0 {
                    let offs = if gid != 0 { offset } else { 0 };
                    charstrings.offset[gid as usize] = (offs + 1) as LOffset; // start at 1
                    t1_decrypt(
                        T1_CHARKEY,
                        &mut charstrings.data[offs as usize..],
                        &s[*p..],
                        len_iv,
                        len,
                    );
                    offset += len - len_iv;
                } else {
                    let l = len as usize;
                    if gid == 0 {
                        charstrings.offset[0] = 1;
                        charstrings.data[..l].copy_from_slice(&s[*p..*p + l]);
                    } else {
                        charstrings.offset[gid as usize] = (offset + 1) as LOffset;
                        charstrings.data[offset as usize..offset as usize + l]
                            .copy_from_slice(&s[*p..*p + l]);
                    }
                    offset += len;
                }
            }
            *p += len as usize;

            let tok = pst_get_token(s, p)?;
            if !match_op(tok.as_ref(), b"ND")? && !match_op(tok.as_ref(), b"|-")? {
                return Ok(-1);
            }
            i += 1;
        }
        if mode != 1 {
            font.cstrings.as_mut().unwrap().offset[count as usize] = (offset + 1) as LOffset;
        }
        font.num_glyphs = count as Card16;

        Ok(0)
    }
    /// `parse_part2` (static, prefixed).
    fn t1_load_parse_part2(
        &mut self,
        font: &mut CffFont,
        s: &[u8],
        p: &mut usize,
        mode: i32,
    ) -> Result<i32> {
        let mut argv = [0.0f64; MAX_ARGS];
        let mut len_iv = 4;

        while *p < s.len() {
            let Some(key) = get_next_key(s, p)? else {
                break;
            };
            let k = key.as_slice();
            if k == b"Subrs" {
                // levIV must appear before Subrs
                if self.t1_load_parse_subrs(font, s, p, len_iv, mode)? < 0 {
                    return Ok(-1);
                }
            } else if k == b"CharStrings" {
                if self.t1_load_parse_charstrings(font, s, p, len_iv, mode)? < 0 {
                    return Ok(-1);
                }
            } else if k == b"lenIV" {
                let argn = parse_nvalue(s, p, &mut argv, 1)?;
                if !check_argn_eq(argn, 1) {
                    return Ok(-1);
                }
                len_iv = argv[0] as i32;
            } else if k == b"BlueValues"
                || k == b"OtherBlues"
                || k == b"FamilyBlues"
                || k == b"FamilyOtherBlues"
                || k == b"StemSnapH"
                || k == b"StemSnapV"
            {
                // Operand values are delta in CFF font dictionary encoding.
                let mut argn = parse_nvalue(s, p, &mut argv, MAX_ARGS as i32)?;
                if !check_argn_ge(argn, 0) {
                    return Ok(-1);
                }
                private0(font).cff_dict_add(k, argn)?;
                while argn > 0 {
                    argn -= 1;
                    let a = argn as usize;
                    let v = if argn == 0 {
                        argv[a]
                    } else {
                        argv[a] - argv[a - 1]
                    };
                    private0(font).cff_dict_set(k, argn, v)?;
                }
            } else if k == b"StdHW"
                || k == b"StdVW"
                || k == b"BlueScale"
                || k == b"BlueShift"
                || k == b"BlueFuzz"
                || k == b"LanguageGroup"
                || k == b"ExpansionFactor"
            {
                // Value of StdHW and StdVW is described as an array in the
                // Type 1 Font Specification but is a number in CFF format.
                let argn = parse_nvalue(s, p, &mut argv, 1)?;
                if !check_argn_eq(argn, 1) {
                    return Ok(-1);
                }
                private0(font).cff_dict_add(k, 1)?;
                private0(font).cff_dict_set(k, 0, argv[0])?;
            } else if k == b"ForceBold" {
                let (argn, v) = parse_bvalue(s, p)?;
                argv[0] = v;
                if !check_argn_eq(argn, 1) {
                    return Ok(-1);
                }
                if argv[0] != 0.0 {
                    private0(font).cff_dict_add(k, 1)?;
                    private0(font).cff_dict_set(k, 0, 1.0)?;
                }
            }
            // MinFeature, RndStemUp, UniqueID, Password ignored.
        }

        Ok(0)
    }
    /// `t1_load_font`: the font as CFF (`mode` 1: metrics only, no
    /// charstrings kept).
    pub fn t1_load_font(
        &mut self,
        enc_vec: Option<&mut [Option<Vec<u8>>]>,
        mode: i32,
        fp: &mut MemFile,
    ) -> Result<Option<CffFont>> {
        fp.rewind();
        // ASCII section
        let buffer = match get_pfb_segment(fp, PFB_SEG_TYPE_ASCII)? {
            Some(b) if !b.is_empty() => b,
            _ => fatal!("Reading PFB (ASCII part) file failed."),
        };

        let mut cff = CffFont::default();
        init_cff_font(&mut cff);

        let mut p = 0;
        if parse_part1(&mut cff, enc_vec, &buffer, &mut p)? < 0 {
            cff.cff_close();
            fatal!("Reading PFB (ASCII part) file failed.");
        }
        drop(buffer);

        // Binary section
        let mut buffer = match get_pfb_segment(fp, PFB_SEG_TYPE_BINARY)? {
            Some(b) if !b.is_empty() => b,
            _ => {
                cff.cff_close();
                fatal!("Reading PFB (BINARY part) file failed.");
            }
        };
        t1_decrypt_in_place(T1_EEKEY, &mut buffer);
        let mut p = 4;
        if self.t1_load_parse_part2(&mut cff, &buffer, &mut p, mode)? < 0 {
            cff.cff_close();
            fatal!("Reading PFB (BINARY part) file failed.");
        }
        drop(buffer);

        cff.cff_update_string();

        // Remaining section ignored.

        Ok(Some(cff))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;

    fn encrypt(key: u16, plain: &[u8]) -> Vec<u8> {
        let mut r = key;
        plain
            .iter()
            .map(|&p| {
                let c = p ^ (r >> 8) as u8;
                r = (u16::from(c).wrapping_add(r))
                    .wrapping_mul(52845)
                    .wrapping_add(22719);
                c
            })
            .collect()
    }

    #[test]
    fn decrypt() {
        let plain = b"abcdHello, charstring";
        let enc = encrypt(T1_CHARKEY, plain);
        let mut dst = [0u8; 17];
        t1_decrypt(T1_CHARKEY, &mut dst, &enc, 4, enc.len() as i32);
        assert_eq!(&dst, b"Hello, charstring");
        let mut buf = encrypt(T1_EEKEY, plain);
        t1_decrypt_in_place(T1_EEKEY, &mut buf);
        assert_eq!(buf, plain);
    }

    #[test]
    fn encoding() {
        let src = b"/Encoding 256 array\n0 1 255 {1 index exch /.notdef put} for\n\
            dup 0 /Gamma put\ndup 65 /A put\ndup 66 /B foo\ndup dup 1 exch 0 get put\n\
            dup dup 10 3 getinterval 20 exch putinterval\nreadonly def\ndup 67 /C put";
        let mut enc: Vec<Option<Vec<u8>>> = vec![None; 256];
        enc[10] = Some(b"ten".to_vec());
        enc[12] = Some(b"twelve".to_vec());
        let mut p = 0;
        assert_eq!(get_next_key(src, &mut p).unwrap().unwrap(), b"Encoding");
        assert_eq!(parse_encoding(Some(&mut enc), src, &mut p).unwrap(), 0);
        assert_eq!(enc[0].as_deref(), Some(&b"Gamma"[..]));
        assert_eq!(enc[65].as_deref(), Some(&b"A"[..]));
        assert_eq!(enc[66], None);
        // dup dup 1 exch 0 get put: code 1 gets code 0's name
        assert_eq!(enc[1].as_deref(), Some(&b"Gamma"[..]));
        assert_eq!(enc[20].as_deref(), Some(&b"ten"[..]));
        assert_eq!(enc[21], None);
        assert_eq!(enc[22].as_deref(), Some(&b"twelve"[..]));
        assert_eq!(enc[67], None);

        let mut p = 0;
        let src = b" StandardEncoding def";
        assert_eq!(parse_encoding(Some(&mut enc), src, &mut p).unwrap(), 0);
        assert_eq!(enc[0], None);
        assert_eq!(enc[0xe1].as_deref(), Some(&b"AE"[..]));
    }

    #[test]
    fn values() {
        let src = b"[-10 -250 1000 900] readonly {0.001 0 0 0.001 0 0} [1 2 /x] 7 true (str)";
        let mut p = 0;
        let mut v = [0.0; MAX_ARGS];
        assert_eq!(parse_nvalue(src, &mut p, &mut v, 4).unwrap(), 4);
        assert_eq!(&v[..4], &[-10.0, -250.0, 1000.0, 900.0]);
        assert_eq!(seek_operator(src, &mut p, b"readonly").unwrap(), 0);
        assert_eq!(parse_nvalue(src, &mut p, &mut v, 6).unwrap(), 6);
        assert_eq!(v[0], 0.001);
        assert_eq!(parse_nvalue(src, &mut p, &mut v, 6).unwrap(), -1);
        // The "]" after the name is a token of its own.
        assert_eq!(parse_nvalue(src, &mut p, &mut v, 1).unwrap(), 0);
        assert_eq!(parse_nvalue(src, &mut p, &mut v, 1).unwrap(), 1);
        assert_eq!(v[0], 7.0);
        assert_eq!(parse_bvalue(src, &mut p).unwrap(), (1, 1.0));
        assert_eq!(
            parse_svalue(src, &mut p).unwrap(),
            (1, Some(b"str".to_vec()))
        );
        assert_eq!(seek_operator(src, &mut p, b"def").unwrap(), -1);
    }

    #[test]
    fn pfb() {
        let mut data = vec![128, 1, 24, 0, 0, 0];
        data.extend_from_slice(b"%!PS-AdobeFont-1.0: X 1\n");
        data.extend_from_slice(&[128, 1, 30, 0, 0, 0]);
        data.extend_from_slice(b"/FontName /Foo-Bar def begin\n");
        data.push(b' ');
        data.extend_from_slice(&[128, 2, 2, 0, 0, 0, 0xaa, 0xbb, 128, 3]);
        let mut fp = MemFile::new(Arc::from(data), b"x.pfb");
        assert_eq!(is_pfb(&mut fp), 1);
        fp.rewind();
        let a = get_pfb_segment(&mut fp, PFB_SEG_TYPE_ASCII)
            .unwrap()
            .unwrap();
        assert_eq!(a.len(), 54);
        assert_eq!(
            get_pfb_segment(&mut fp, PFB_SEG_TYPE_BINARY)
                .unwrap()
                .unwrap(),
            [0xaa, 0xbb]
        );
        // The FontName is looked for after "begin".
        assert_eq!(t1_get_fontname(&mut fp).unwrap(), (0, Vec::new()));
    }
}
