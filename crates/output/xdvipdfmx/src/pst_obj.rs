//! pst_obj.c, pst_obj.h: [`PstObj`]'s functions and the parsers of each
//! type. Parsers are `(s: &[u8], p: &mut usize)` (see pst.rs).

use crate::prelude::*;
use crate::pst::{PstObj, PstType};

/// `pst_const_null`.
pub const PST_CONST_NULL: &[u8] = b"null";
/// `pst_const_mark`.
pub const PST_CONST_MARK: &[u8] = b"mark";

/// `pst_new_mark`.
pub fn pst_new_mark() -> PstObj {
    PstObj::Mark
}

/// `pst_release_obj`.
pub fn pst_release_obj(obj: PstObj) {
    drop(obj);
}

use crate::dpxutil::{skip_white_spaces, xtoi};
use crate::fmt::Buf;
use crate::pst::{
    PST_NAME_LEN_MAX, PST_STRING_LEN_MAX, PST_TYPE_BOOLEAN, PST_TYPE_INTEGER, PST_TYPE_MARK,
    PST_TYPE_NAME, PST_TYPE_NULL, PST_TYPE_REAL, PST_TYPE_STRING, PST_TYPE_UNKNOWN, at, is_delim,
    is_space, pst_token_end,
};

/// `TYPE_ERROR()`.
fn type_error<T>() -> Result<T> {
    fatal!("Operation not defined for this type of object.")
}

#[allow(non_snake_case)]
impl PstObj {
    /// `pst_type_of`.
    pub fn pst_type_of(&self) -> PstType {
        match self {
            PstObj::Unknown(_) => PST_TYPE_UNKNOWN,
            PstObj::Null => PST_TYPE_NULL,
            PstObj::Boolean(_) => PST_TYPE_BOOLEAN,
            PstObj::Integer(_) => PST_TYPE_INTEGER,
            PstObj::Real(_) => PST_TYPE_REAL,
            PstObj::String(_) => PST_TYPE_STRING,
            PstObj::Name(_) => PST_TYPE_NAME,
            PstObj::Mark => PST_TYPE_MARK,
        }
    }
    /// `pst_length_of`.
    pub fn pst_length_of(&self) -> Result<i32> {
        match self {
            // pst_boolean_length, pst_integer_length, pst_real_length
            PstObj::Boolean(_) | PstObj::Integer(_) | PstObj::Real(_) => return type_error()?,
            PstObj::Name(v) => Ok(v.len() as i32),
            PstObj::String(v) => Ok(v.len() as i32),
            PstObj::Null | PstObj::Mark => return type_error()?,
            PstObj::Unknown(v) => Ok(v.len() as i32),
        }
    }
    /// `pst_getIV`.
    pub fn pst_getIV(&self) -> Result<i32> {
        match self {
            PstObj::Boolean(v) => Ok(i32::from(*v)),
            PstObj::Integer(v) => Ok(*v),
            PstObj::Real(v) => Ok(*v as i32),
            PstObj::Name(_) => return type_error()?,
            PstObj::String(v) => Ok(pst_string_RV(v)? as i32),
            PstObj::Null | PstObj::Mark => return type_error()?,
            PstObj::Unknown(_) => {
                fatal!("Cannot convert object of type UNKNOWN to integer value.")
            }
        }
    }
    /// `pst_getRV`.
    pub fn pst_getRV(&self) -> Result<f64> {
        match self {
            PstObj::Boolean(v) => Ok(f64::from(*v)),
            PstObj::Integer(v) => Ok(f64::from(*v)),
            PstObj::Real(v) => Ok(*v),
            PstObj::Name(_) => return type_error()?,
            PstObj::String(v) => pst_string_RV(v),
            PstObj::Null | PstObj::Mark => return type_error()?,
            PstObj::Unknown(_) => fatal!("Cannot convert object of type UNKNOWN to real value."),
        }
    }
    /// `pst_getSV`: a new string (C: a NUL-terminated copy; its length is
    /// `pst_length_of`). None for an empty unknown token; null and mark
    /// are a type error.
    pub fn pst_getSV(&self) -> Result<Option<Vec<u8>>> {
        match self {
            PstObj::Boolean(v) => Ok(Some(if *v != 0 {
                b"true".to_vec()
            } else {
                b"false".to_vec()
            })),
            PstObj::Integer(v) => {
                let mut b = Buf::new();
                b.int(*v);
                Ok(Some(b.0))
            }
            PstObj::Real(v) => Ok(Some(sprintf_g(*v, 5))),
            PstObj::Name(v) | PstObj::String(v) => Ok(Some(v.clone())),
            PstObj::Null | PstObj::Mark => return type_error()?,
            PstObj::Unknown(v) => {
                if !v.is_empty() {
                    Ok(Some(v.clone()))
                } else {
                    Ok(None)
                }
            }
        }
    }
    /// `pst_data_ptr`: the bytes of a name, string or unknown token
    /// (C points into the object; for numbers and booleans C points at
    /// the binary value: empty here; null and mark are a type error).
    pub fn pst_data_ptr(&self) -> Result<&[u8]> {
        match self {
            PstObj::Boolean(_) | PstObj::Integer(_) | PstObj::Real(_) => Ok(&[]),
            PstObj::Name(v) | PstObj::String(v) | PstObj::Unknown(v) => Ok(v),
            PstObj::Null | PstObj::Mark => return type_error()?,
        }
    }
}

/// `pst_string_RV` (static): the string read as a number.
#[allow(non_snake_case)]
fn pst_string_RV(v: &[u8]) -> Result<f64> {
    let mut p = 0;
    let nobj = pst_parse_number(v, &mut p);
    match nobj {
        Some(n) if p == v.len() => n.pst_getRV(),
        _ => fatal!("Cound not convert string to real value."),
    }
}

/// C's `sprintf(buf, "%.<prec>g", v)`.
pub fn sprintf_g(v: f64, prec: usize) -> Vec<u8> {
    let mut b = Buf::new();
    if v.is_nan() {
        b.extend(if v.is_sign_negative() {
            b"-nan"
        } else {
            b"nan"
        });
        return b.0;
    }
    if v.is_infinite() {
        b.extend(if v < 0.0 { b"-inf" } else { b"inf" });
        return b.0;
    }
    let p = if prec == 0 { 1 } else { prec };
    // The exponent X of the %e conversion with precision P - 1.
    let e = format!("{:.*e}", p - 1, v);
    let epos = e.rfind('e').unwrap_or(e.len());
    let x: i32 = e[epos + 1..].parse().unwrap_or(0);
    if (p as i32) > x && x >= -4 {
        let f = format!("{:.*}", (p as i32 - 1 - x) as usize, v);
        b.extend(strip_zeros(f.as_bytes()));
    } else {
        b.extend(strip_zeros(&e.as_bytes()[..epos]));
        b.push(b'e');
        b.push(if x < 0 { b'-' } else { b'+' });
        let ax = x.unsigned_abs();
        if ax < 10 {
            b.push(b'0');
        }
        b.uint(ax);
    }
    b.0
}

/// `%g` drops trailing zeros of the fraction, and a trailing point.
fn strip_zeros(s: &[u8]) -> &[u8] {
    if !s.contains(&b'.') {
        return s;
    }
    let mut n = s.len();
    while n > 0 && s[n - 1] == b'0' {
        n -= 1;
    }
    if n > 0 && s[n - 1] == b'.' {
        n -= 1;
    }
    &s[..n]
}

/// `pst_parse_boolean`.
pub fn pst_parse_boolean(s: &[u8], p: &mut usize) -> Option<PstObj> {
    if *p + 4 <= s.len() && &s[*p..*p + 4] == b"true" && pst_token_end(s, *p + 4) {
        *p += 4;
        Some(PstObj::Boolean(1))
    } else if *p + 5 <= s.len() && &s[*p..*p + 5] == b"false" && pst_token_end(s, *p + 5) {
        *p += 5;
        Some(PstObj::Boolean(0))
    } else {
        None
    }
}

/// `pst_parse_null`.
pub fn pst_parse_null(s: &[u8], p: &mut usize) -> Option<PstObj> {
    if *p + 4 <= s.len() && &s[*p..*p + 4] == b"null" && pst_token_end(s, *p + 4) {
        *p += 4;
        Some(PstObj::Null)
    } else {
        None
    }
}

/// C's `isspace`.
fn c_isspace(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// glibc's `strtol(s + p, &end, base)` with `errno`: the `long` value
/// (64-bit), the end position, and whether it overflowed (`ERANGE`).
/// No digits: 0 and the end is `p`. (Base 16's `0x` prefix is not
/// taken: pst_parse_number never asks for it.)
fn strtol_errno(s: &[u8], p: usize, base: u32) -> (i64, usize, bool) {
    let mut i = p;
    while c_isspace(at(s, i)) {
        i += 1;
    }
    let mut neg = false;
    if at(s, i) == b'+' || at(s, i) == b'-' {
        neg = at(s, i) == b'-';
        i += 1;
    }
    let start = i;
    // The magnitude, as an unsigned long, as glibc accumulates it.
    let mut v: u64 = 0;
    let mut overflow = false;
    loop {
        let d = match at(s, i) {
            c @ b'0'..=b'9' => u32::from(c - b'0'),
            c @ b'a'..=b'z' => u32::from(c - b'a') + 10,
            c @ b'A'..=b'Z' => u32::from(c - b'A') + 10,
            _ => break,
        };
        if d >= base {
            break;
        }
        match v
            .checked_mul(u64::from(base))
            .and_then(|x| x.checked_add(u64::from(d)))
        {
            Some(x) => v = x,
            None => overflow = true,
        }
        i += 1;
    }
    if i == start {
        return (0, p, false);
    }
    let limit = if neg {
        (i64::MAX as u64) + 1
    } else {
        i64::MAX as u64
    };
    if overflow || v > limit {
        return (if neg { i64::MIN } else { i64::MAX }, i, true);
    }
    let r = if neg {
        (v as i64).wrapping_neg()
    } else {
        v as i64
    };
    (r, i, false)
}

/// glibc's `strtod(s + p, &end)` with `errno` (`ERANGE` on overflow and
/// on underflow to a subnormal or zero).
fn strtod_errno(s: &[u8], p: usize) -> (f64, usize, bool) {
    let (v, n) = crate::fmt::strtod(&s[p.min(s.len())..]);
    if n == 0 {
        return (v, p, false);
    }
    let text = &s[p..p + n];
    let mut erange = v.is_infinite() || (v != 0.0 && v.abs() < f64::MIN_POSITIVE);
    if v == 0.0 {
        // Underflow: a mantissa with a non-zero digit read as zero.
        let mant_end = text
            .iter()
            .position(|&c| c == b'e' || c == b'E')
            .unwrap_or(text.len());
        if text[..mant_end].iter().any(|&c| (b'1'..=b'9').contains(&c)) {
            erange = true;
        }
    }
    (v, p + n, erange)
}

/// `pst_parse_number`: integer, real, or radix (`16#ff`) number. C reads
/// with `strtol`/`strtod`, which need `*inbufend == 0`.
pub fn pst_parse_number(s: &[u8], p: &mut usize) -> Option<PstObj> {
    let (l, mut cur, errno) = strtol_errno(s, *p, 10);
    let lval = l as i32;
    if errno || at(s, cur) == b'.' || at(s, cur) == b'e' || at(s, cur) == b'E' {
        // real
        let (dval, cur, errno) = strtod_errno(s, *p);
        if !errno && pst_token_end(s, cur) {
            *p = cur;
            return Some(PstObj::Real(dval));
        }
    } else if cur != *p && pst_token_end(s, cur) {
        // integer
        *p = cur;
        return Some(PstObj::Integer(lval));
    } else if (2..=36).contains(&lval)
        && at(s, cur) == b'#'
        && {
            cur += 1;
            at(s, cur).is_ascii_alphanumeric()
        }
        && (lval != 16 || (at(s, cur + 1) != b'x' && at(s, cur + 1) != b'X'))
    {
        // integer with radix
        // Can the base have a (plus) sign? I think yes.
        let (l, cur, errno) = strtol_errno(s, cur, lval as u32);
        if !errno && pst_token_end(s, cur) {
            *p = cur;
            return Some(PstObj::Integer(l as i32));
        }
    }
    // error
    None
}

/// `pst_name_is_valid` (static; `#if 0` in C).
fn pst_name_is_valid(name: &[u8]) -> bool {
    const VALID_CHARS: &[u8] =
        b"!\"#$&'*+,-.0123456789:;=?@ABCDEFGHIJKLMNOPQRSTUVWXYZ\\^_`abcdefghijklmnopqrstuvwxyz|~";
    name.iter().all(|c| VALID_CHARS.contains(c))
}

/// `putxpair` (static; `#if 0` in C): the two hex digits of `c`
/// appended; the bytes written (2).
fn putxpair(c: u8, s: &mut Vec<u8>) -> i32 {
    let hi = c >> 4;
    let lo = c & 0x0f;
    s.push(if hi < 10 { hi + b'0' } else { hi + b'7' });
    s.push(if lo < 10 { lo + b'0' } else { lo + b'7' });
    2
}

/// `pst_name_encode` (static; `#if 0` in C).
fn pst_name_encode(name: &[u8]) -> Vec<u8> {
    let mut len = name.len();
    if len > PST_NAME_LEN_MAX {
        warn!("Input string too long for name object. String will be truncated.");
        len = PST_NAME_LEN_MAX;
    }
    let mut encoded = Vec::with_capacity(3 * len);
    for &c in &name[..len] {
        if !(b'!'..=b'~').contains(&c) || c == b'#' || is_delim(c) || is_space(c) {
            encoded.push(b'#');
            putxpair(c, &mut encoded);
        } else {
            encoded.push(c);
        }
    }
    encoded
}

/// `getxpair` (static): the byte of two hex digits at `s[*p..]`, or < 0.
fn getxpair(s: &[u8], p: &mut usize) -> i32 {
    let hi = xtoi(at(s, *p));
    if hi < 0 {
        return hi;
    }
    *p += 1;
    let lo = xtoi(at(s, *p));
    if lo < 0 {
        return lo;
    }
    *p += 1;
    (hi << 4) | lo
}

/// `pst_parse_name` (the `/` is required).
pub fn pst_parse_name(s: &[u8], p: &mut usize) -> Option<PstObj> {
    let mut wbuf: Vec<u8> = Vec::new();
    let mut cur = *p;
    let mut len: usize = 0;

    if at(s, cur) != b'/' {
        return None;
    }
    cur += 1;

    while !pst_token_end(s, cur) {
        let mut c = s[cur];
        cur += 1;
        if c == b'#' {
            if cur + 2 >= s.len() {
                warn!("Premature end of input name string.");
                break;
            }
            let val = getxpair(s, &mut cur);
            if val <= 0 {
                warn!("Invalid char for name object. (ignored)");
                continue;
            }
            c = val as u8;
        }
        if len < PST_NAME_LEN_MAX {
            wbuf.push(c);
        }
        len += 1;
    }

    if len > PST_NAME_LEN_MAX {
        warn!("String too long for name object. Output will be truncated.");
    }

    *p = cur;
    Some(PstObj::Name(wbuf))
}

/// `pst_parse_string`: literal `(…)` or hex `<…>`.
pub fn pst_parse_string(s: &[u8], p: &mut usize) -> Result<Option<PstObj>> {
    if *p + 2 >= s.len() {
        Ok(None)
    } else if s[*p] == b'(' {
        match pst_string_parse_literal(s, p) {
            Some(v) => Ok(Some(PstObj::String(v))),
            None => fatal!("NULL pointer data for object type: {}", PST_TYPE_STRING),
        }
    } else if s[*p] == b'<' && s[*p + 1] == b'~' {
        fatal!("ASCII85 string not supported yet.")
    } else if s[*p] == b'<' {
        match pst_string_parse_hex(s, p) {
            Some(v) => Ok(Some(PstObj::String(v))),
            None => fatal!("NULL pointer data for object type: {}", PST_TYPE_STRING),
        }
    } else {
        Ok(None)
    }
}

/// `ostrtouc` (static): up to three octal digits; overflowed value is
/// set to invalid char.
fn ostrtouc(s: &[u8], p: &mut usize, valid: &mut u8) -> u8 {
    let mut cur = *p;
    let mut val: u32 = 0;
    while cur < s.len() && cur < *p + 3 && (b'0'..=b'7').contains(&s[cur]) {
        val = (val << 3) | u32::from(s[cur] - b'0');
        cur += 1;
    }
    *valid = if val > 255 || cur == *p { 0 } else { 1 };
    *p = cur;
    val as u8
}

/// `esctouc` (static).
fn esctouc(s: &[u8], p: &mut usize, valid: &mut u8) -> u8 {
    let escaped = at(s, *p);
    *valid = 1;
    match escaped {
        // Backslash, unbalanced paranthes
        b'\\' | b')' | b'(' => {
            *p += 1;
            escaped
        }
        // Other escaped char
        b'n' => {
            *p += 1;
            b'\n'
        }
        b'r' => {
            *p += 1;
            b'\r'
        }
        b't' => {
            *p += 1;
            b'\t'
        }
        b'b' => {
            *p += 1;
            0x08
        }
        b'f' => {
            *p += 1;
            0x0c
        }
        // An end-of-line marker preceeded by backslash is not part of a
        // literal string
        b'\r' => {
            *valid = 0;
            *p += if *p + 1 < s.len() && s[*p + 1] == b'\n' {
                2
            } else {
                1
            };
            0
        }
        b'\n' => {
            *valid = 0;
            *p += 1;
            0
        }
        // Possibly octal notion
        _ => ostrtouc(s, p, valid),
    }
}

/// `pst_string_parse_literal` (static).
fn pst_string_parse_literal(s: &[u8], p: &mut usize) -> Option<Vec<u8>> {
    let mut wbuf: Vec<u8> = Vec::new();
    let mut cur = *p;
    let mut c: u8 = 0;
    let mut balance = 1;

    if cur + 2 > s.len() || s[cur] != b'(' {
        return None;
    }

    cur += 1;
    while cur < s.len() && wbuf.len() < PST_STRING_LEN_MAX && balance > 0 {
        c = s[cur];
        cur += 1;
        match c {
            b'\\' => {
                let mut valid = 0;
                let unescaped = esctouc(s, &mut cur, &mut valid);
                if valid != 0 {
                    wbuf.push(unescaped);
                }
            }
            b'(' => {
                balance += 1;
                wbuf.push(b'(');
            }
            b')' => {
                balance -= 1;
                if balance > 0 {
                    wbuf.push(b')');
                }
            }
            // An end-of-line marker (\n, \r or \r\n), not preceeded by a
            // backslash, must be converted to single \n.
            b'\r' => {
                if cur < s.len() && s[cur] == b'\n' {
                    cur += 1;
                }
                wbuf.push(b'\n');
            }
            _ => wbuf.push(c),
        }
    }
    if c != b')' {
        return None;
    }

    *p = cur;
    Some(wbuf)
}

/// `pst_string_parse_hex` (static).
fn pst_string_parse_hex(s: &[u8], p: &mut usize) -> Option<Vec<u8>> {
    let mut wbuf: Vec<u8> = Vec::new();
    let mut cur = *p;

    if cur + 2 > s.len() || s[cur] != b'<' || (s[cur] == b'<' && s[cur + 1] == b'<') {
        return None;
    }

    cur += 1;
    // PDF Reference does not specify how to treat invalid char
    while cur < s.len() && wbuf.len() < PST_STRING_LEN_MAX {
        skip_white_spaces(s, &mut cur);
        if at(s, cur) == b'>' {
            break;
        }
        let mut hi = xtoi(at(s, cur));
        cur += 1;
        if hi < 0 {
            warn!(
                "Invalid char for hex string <{:x}> treated as <0>.",
                at(s, cur - 1)
            );
            hi = 0;
        }
        skip_white_spaces(s, &mut cur);
        if at(s, cur) == b'>' {
            break;
        }
        // 0 is appended if final hex digit is missing
        let mut lo = if cur < s.len() {
            cur += 1;
            xtoi(s[cur - 1])
        } else {
            0
        };
        if lo < 0 {
            warn!(
                "Invalid char for hex string <{:x}> treated as <0>.",
                at(s, cur - 1)
            );
            lo = 0;
        }
        wbuf.push(((hi << 4) | lo) as u8);
    }
    let c = at(s, cur);
    cur += 1;
    if c != b'>' {
        return None;
    }

    *p = cur;
    Some(wbuf)
}
