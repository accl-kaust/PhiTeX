//! pst.c, pst.h: a PostScript tokenizer (for Type 1 fonts).
//!
//! `pst_obj` is the enum [`PstObj`] (its functions, from pst_obj.c, are in
//! pst_obj.rs). Parsers `(unsigned char **inbuf, unsigned char *inbufend)`
//! are `(s: &[u8], p: &mut usize)` with `s` ending at `inbufend`; C
//! requires `*inbufend == 0`, so a read at `s.len()` is a 0 byte.

use crate::prelude::*;

/// `pst_type`.
pub type PstType = i32;

pub const PST_TYPE_UNKNOWN: PstType = -1;
pub const PST_TYPE_NULL: PstType = 0;
pub const PST_TYPE_BOOLEAN: PstType = 1;
pub const PST_TYPE_INTEGER: PstType = 2;
pub const PST_TYPE_REAL: PstType = 3;
pub const PST_TYPE_STRING: PstType = 5;
pub const PST_TYPE_NAME: PstType = 6;
pub const PST_TYPE_MARK: PstType = 7;

pub const PST_NAME_LEN_MAX: usize = 127;
pub const PST_STRING_LEN_MAX: usize = 32767;
pub const PST_MAX_DIGITS: usize = 10;
pub const PST_TOKEN_LEN_MAX: usize = PST_STRING_LEN_MAX;

/// `pst_obj`: its type and data.
#[derive(Clone, Debug, PartialEq)]
pub enum PstObj {
    /// `PST_TYPE_UNKNOWN`: an operator or other token, its text.
    Unknown(Vec<u8>),
    /// `PST_TYPE_NULL`.
    Null,
    /// `PST_TYPE_BOOLEAN` (C `char`).
    Boolean(i8),
    /// `PST_TYPE_INTEGER`.
    Integer(i32),
    /// `PST_TYPE_REAL`.
    Real(f64),
    /// `PST_TYPE_STRING`.
    String(Vec<u8>),
    /// `PST_TYPE_NAME` (without the `/`).
    Name(Vec<u8>),
    /// `PST_TYPE_MARK`.
    Mark,
}

impl Default for PstObj {
    fn default() -> Self {
        PstObj::Null
    }
}

impl PstObj {
    /// `PST_NULLTYPE`.
    pub fn is_null(&self) -> bool {
        self.pst_type_of() == PST_TYPE_NULL
    }
    /// `PST_BOOLEANTYPE`.
    pub fn is_boolean(&self) -> bool {
        self.pst_type_of() == PST_TYPE_BOOLEAN
    }
    /// `PST_NAMETYPE`.
    pub fn is_name(&self) -> bool {
        self.pst_type_of() == PST_TYPE_NAME
    }
    /// `PST_STRINGTYPE`.
    pub fn is_string(&self) -> bool {
        self.pst_type_of() == PST_TYPE_STRING
    }
    /// `PST_INTEGERTYPE`.
    pub fn is_integer(&self) -> bool {
        self.pst_type_of() == PST_TYPE_INTEGER
    }
    /// `PST_REALTYPE`.
    pub fn is_real(&self) -> bool {
        self.pst_type_of() == PST_TYPE_REAL
    }
    /// `PST_NUMBERTYPE`.
    pub fn is_number(&self) -> bool {
        self.is_integer() || self.is_real()
    }
    /// `PST_MARKTYPE`.
    pub fn is_mark(&self) -> bool {
        self.pst_type_of() == PST_TYPE_MARK
    }
    /// `PST_UNKNOWNTYPE`.
    pub fn is_unknown(&self) -> bool {
        self.pst_type_of() < 0
    }
}

/// dpxutil.h's `is_space` (`\0` is a space).
#[must_use]
pub fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | 0x0c | b'\r' | b'\n' | 0)
}

/// dpxutil.h's `is_delim` (with `{` and `}`, unlike pdfparse's).
#[must_use]
pub fn is_delim(c: u8) -> bool {
    matches!(
        c,
        b'(' | b')' | b'/' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'%'
    )
}

/// `*p` (C reads the NUL at `inbufend`; past the end is 0 here).
#[inline]
pub(crate) fn at(s: &[u8], p: usize) -> u8 {
    s.get(p).copied().unwrap_or(0)
}

/// `PST_TOKEN_END(s, e)`: at the end, a delimiter or a space.
pub fn pst_token_end(s: &[u8], p: usize) -> bool {
    p == s.len() || is_delim(at(s, p)) || is_space(at(s, p))
}

/// `pst_parse_any` (static): a token up to the next delimiter, as
/// `Unknown`.
fn pst_parse_any(s: &[u8], p: &mut usize) -> Option<PstObj> {
    let mut cur = *p;
    while cur < s.len() && !pst_token_end(s, cur) {
        cur += 1;
    }
    let data = s[*p..cur].to_vec();
    *p = cur;
    Some(PstObj::Unknown(data))
}

/// `skip_line` (static).
fn skip_line(s: &[u8], p: &mut usize) {
    while *p < s.len() && s[*p] != b'\n' && s[*p] != b'\r' {
        *p += 1;
    }
    if *p < s.len() && s[*p] == b'\r' {
        *p += 1;
    }
    if *p < s.len() && s[*p] == b'\n' {
        *p += 1;
    }
}

/// `skip_comments` (static).
fn skip_comments(s: &[u8], p: &mut usize) {
    while *p < s.len() && s[*p] == b'%' {
        skip_line(s, p);
        crate::dpxutil::skip_white_spaces(s, p);
    }
}

/// `pst_get_token`: the next token, or none at the end.
pub fn pst_get_token(s: &[u8], p: &mut usize) -> Option<PstObj> {
    use crate::pst_obj::{
        pst_new_mark, pst_parse_boolean, pst_parse_name, pst_parse_null, pst_parse_number,
        pst_parse_string,
    };
    let mut obj: Option<PstObj> = None;

    assert!(*p <= s.len());

    crate::dpxutil::skip_white_spaces(s, p);
    skip_comments(s, p);
    if *p >= s.len() {
        return None;
    }
    let c = s[*p];
    match c {
        b'/' => {
            obj = pst_parse_name(s, p);
        }
        b'[' | b'{' => {
            // This is wrong
            obj = Some(pst_new_mark());
            *p += 1;
        }
        b'<' => {
            if *p + 1 >= s.len() {
                return None;
            }
            let c = s[*p + 1];
            if c == b'<' {
                obj = Some(pst_new_mark());
                *p += 2;
            } else if c.is_ascii_hexdigit() {
                obj = pst_parse_string(s, p);
            } else if c == b'~' {
                // ASCII85
                obj = pst_parse_string(s, p);
            }
        }
        b'(' => {
            obj = pst_parse_string(s, p);
        }
        b'>' => {
            if *p + 1 >= s.len() || s[*p + 1] != b'>' {
                error!("Unexpected end of ASCII hex string marker.");
            } else {
                obj = Some(PstObj::Unknown(b">>".to_vec()));
                *p += 2;
            }
        }
        b']' | b'}' => {
            obj = Some(PstObj::Unknown(vec![c]));
            *p += 1;
        }
        _ => {
            if c == b't' || c == b'f' {
                obj = pst_parse_boolean(s, p);
            } else if c == b'n' {
                obj = pst_parse_null(s, p);
            } else if c == b'+' || c == b'-' || c.is_ascii_digit() || c == b'.' {
                obj = pst_parse_number(s, p);
            }
        }
    }

    if obj.is_none() {
        obj = pst_parse_any(s, p);
    }

    obj
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &[u8]) -> Vec<PstObj> {
        let mut p = 0;
        let mut v = Vec::new();
        while p < s.len() {
            match pst_get_token(s, &mut p) {
                Some(t) => v.push(t),
                None => break,
            }
        }
        v
    }

    #[test]
    fn tokens() {
        let t = toks(b"%!PS-AdobeFont\n/FontName /LMRoman10-Regular def\n/a#41 [1 -2.5 16#ff 8#9]{}<< >> (a\\(b\\)\\101\r\n) <414 2> true false null 1e3 .5 -.5 4294967297 99999999999999999999 currentfile");
        assert_eq!(
            t,
            vec![
                PstObj::Name(b"FontName".to_vec()),
                PstObj::Name(b"LMRoman10-Regular".to_vec()),
                PstObj::Unknown(b"def".to_vec()),
                PstObj::Name(b"aA".to_vec()),
                PstObj::Mark,
                PstObj::Integer(1),
                PstObj::Real(-2.5),
                PstObj::Integer(255),
                PstObj::Unknown(b"8#9".to_vec()),
                PstObj::Unknown(b"]".to_vec()),
                PstObj::Mark,
                PstObj::Unknown(b"}".to_vec()),
                PstObj::Mark,
                PstObj::Unknown(b">>".to_vec()),
                PstObj::String(b"a(b)A\n".to_vec()),
                PstObj::String(b"AB".to_vec()),
                PstObj::Boolean(1),
                PstObj::Boolean(0),
                PstObj::Null,
                PstObj::Real(1000.0),
                PstObj::Real(0.5),
                // strtol takes no digits and stops at '-': not a number
                PstObj::Unknown(b"-.5".to_vec()),
                // a long, truncated to int
                PstObj::Integer(1),
                PstObj::Real(1e20),
                PstObj::Unknown(b"currentfile".to_vec()),
            ]
        );
    }

    #[test]
    fn sv() {
        assert_eq!(PstObj::Real(0.001).pst_getSV().unwrap(), b"0.001");
        assert_eq!(PstObj::Real(123456.0).pst_getSV().unwrap(), b"1.2346e+05");
        assert_eq!(PstObj::Real(-2.5).pst_getSV().unwrap(), b"-2.5");
        assert_eq!(PstObj::Real(1e-5).pst_getSV().unwrap(), b"1e-05");
        assert_eq!(PstObj::Integer(-7).pst_getSV().unwrap(), b"-7");
        assert_eq!(PstObj::String(b"12".to_vec()).pst_getIV(), 12);
    }
}
