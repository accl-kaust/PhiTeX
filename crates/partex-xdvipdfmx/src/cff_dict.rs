//! cff_dict.c, cff_dict.h: CFF DICT data.
//!
//! C's `stack_top`/`arg_stack` statics are reset at the start of each
//! `cff_dict_unpack` and only read below `stack_top`, so they are the
//! per-call [`DictStack`], not state. `cff_dict_update(cff->topdict, cff)`
//! in C: here take the dict out of the font first
//! (`let mut d = cff.topdict.take().unwrap(); cff_dict_update(&mut d, cff); cff.topdict = Some(d);`).

use crate::cff::*;
use crate::prelude::*;

pub const CFF_NOMINALWIDTHX_DEFAULT: f64 = 0.0;
pub const CFF_DEFAULTWIDTHX_DEFAULT: f64 = 0.0;

pub const CFF_PARSE_OK: i32 = 0;
pub const CFF_ERROR_PARSE_ERROR: i32 = -1;
pub const CFF_ERROR_STACK_OVERFLOW: i32 = -2;
pub const CFF_ERROR_STACK_UNDERFLOW: i32 = -3;
pub const CFF_ERROR_STACK_RANGECHECK: i32 = -4;

pub const DICT_ENTRY_MAX: i32 = 16;
pub const CFF_DICT_STACK_LIMIT: usize = 64;
pub const CFF_LAST_DICT_OP1: usize = 22;
pub const CFF_LAST_DICT_OP2: usize = 39;
pub const CFF_LAST_DICT_OP: usize = CFF_LAST_DICT_OP1 + CFF_LAST_DICT_OP2;

/// `dict_operator` (cff_dict.c): opname (none for the escape and reserved slots) and argument type.
pub static DICT_OPERATOR: [(Option<&[u8]>, i32); CFF_LAST_DICT_OP] = [
    (Some(b"version".as_slice()), CFF_TYPE_SID),
    (Some(b"Notice".as_slice()), CFF_TYPE_SID),
    (Some(b"FullName".as_slice()), CFF_TYPE_SID),
    (Some(b"FamilyName".as_slice()), CFF_TYPE_SID),
    (Some(b"Weight".as_slice()), CFF_TYPE_SID),
    (Some(b"FontBBox".as_slice()), CFF_TYPE_ARRAY),
    (Some(b"BlueValues".as_slice()), CFF_TYPE_DELTA),
    (Some(b"OtherBlues".as_slice()), CFF_TYPE_DELTA),
    (Some(b"FamilyBlues".as_slice()), CFF_TYPE_DELTA),
    (Some(b"FamilyOtherBlues".as_slice()), CFF_TYPE_DELTA),
    (Some(b"StdHW".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"StdVW".as_slice()), CFF_TYPE_NUMBER),
    (None, -1),
    (Some(b"UniqueID".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"XUID".as_slice()), CFF_TYPE_ARRAY),
    (Some(b"charset".as_slice()), CFF_TYPE_OFFSET),
    (Some(b"Encoding".as_slice()), CFF_TYPE_OFFSET),
    (Some(b"CharStrings".as_slice()), CFF_TYPE_OFFSET),
    (Some(b"Private".as_slice()), CFF_TYPE_SZOFF),
    (Some(b"Subrs".as_slice()), CFF_TYPE_OFFSET),
    (Some(b"defaultWidthX".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"nominalWidthX".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"Copyright".as_slice()), CFF_TYPE_SID),
    (Some(b"IsFixedPitch".as_slice()), CFF_TYPE_BOOLEAN),
    (Some(b"ItalicAngle".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"UnderlinePosition".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"UnderlineThickness".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"PaintType".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"CharstringType".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"FontMatrix".as_slice()), CFF_TYPE_ARRAY),
    (Some(b"StrokeWidth".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"BlueScale".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"BlueShift".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"BlueFuzz".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"StemSnapH".as_slice()), CFF_TYPE_DELTA),
    (Some(b"StemSnapV".as_slice()), CFF_TYPE_DELTA),
    (Some(b"ForceBold".as_slice()), CFF_TYPE_BOOLEAN),
    (None, -1),
    (None, -1),
    (Some(b"LanguageGroup".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"ExpansionFactor".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"InitialRandomSeed".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"SyntheticBase".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"PostScript".as_slice()), CFF_TYPE_SID),
    (Some(b"BaseFontName".as_slice()), CFF_TYPE_SID),
    (Some(b"BaseFontBlend".as_slice()), CFF_TYPE_DELTA),
    (None, -1),
    (None, -1),
    (None, -1),
    (None, -1),
    (None, -1),
    (None, -1),
    (Some(b"ROS".as_slice()), CFF_TYPE_ROS),
    (Some(b"CIDFontVersion".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"CIDFontRevision".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"CIDFontType".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"CIDCount".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"UIDBase".as_slice()), CFF_TYPE_NUMBER),
    (Some(b"FDArray".as_slice()), CFF_TYPE_OFFSET),
    (Some(b"FDSelect".as_slice()), CFF_TYPE_OFFSET),
    (Some(b"FontName".as_slice()), CFF_TYPE_SID),
];

/// `stack_top`, `arg_stack` (C statics; per unpack here).
#[derive(Clone, Debug)]
pub struct DictStack {
    pub stack_top: i32,
    pub arg_stack: [f64; CFF_DICT_STACK_LIMIT],
}

impl Default for DictStack {
    fn default() -> Self {
        DictStack {
            stack_top: 0,
            arg_stack: [0.0; CFF_DICT_STACK_LIMIT],
        }
    }
}

/// mfileio.h's `WORK_BUFFER_SIZE` (`get_real` reads into `work_buffer`).
const WORK_BUFFER_SIZE: usize = 1024;

/// `cff_new_dict`.
pub fn cff_new_dict() -> CffDict {
    CffDict {
        max: DICT_ENTRY_MAX,
        count: 0,
        entries: Vec::with_capacity(DICT_ENTRY_MAX as usize),
    }
}

/// `cff_release_dict`.
pub fn cff_release_dict(dict: CffDict) {
    drop(dict);
}

/// `data[p]`, or 0 past the end (C reads on past `endptr` there).
fn at(data: &[u8], p: usize) -> u8 {
    data.get(p).copied().unwrap_or(0)
}

/// `get_integer` (static): `*status` set on error.
fn get_integer(data: &[u8], p: &mut usize, status: &mut i32) -> f64 {
    let end = data.len();
    let mut result: i32 = 0;

    let b0 = at(data, *p);
    *p += 1;
    if b0 == 28 && *p + 2 < end {
        // shortint
        let b1 = data[*p];
        let b2 = data[*p + 1];
        *p += 2;
        result = i32::from(b1) * 256 + i32::from(b2);
        if result > 0x7fff {
            result -= 0x10000;
        }
    } else if b0 == 29 && *p + 4 < end {
        // longint
        result = i32::from(data[*p]);
        *p += 1;
        if result > 0x7f {
            result -= 0x100;
        }
        for _ in 0..3 {
            result = result.wrapping_mul(256).wrapping_add(i32::from(data[*p]));
            *p += 1;
        }
    } else if (32..=246).contains(&b0) {
        // int (1)
        result = i32::from(b0) - 139;
    } else if (247..=250).contains(&b0) {
        // int (2)
        let b1 = at(data, *p);
        *p += 1;
        result = (i32::from(b0) - 247) * 256 + i32::from(b1) + 108;
    } else if (251..=254).contains(&b0) {
        let b1 = at(data, *p);
        *p += 1;
        result = -(i32::from(b0) - 251) * 256 - i32::from(b1) - 108;
    } else {
        *status = CFF_ERROR_PARSE_ERROR;
    }

    f64::from(result)
}

/// glibc's `strtod` with `errno == ERANGE`: the result overflowed, or a
/// non-zero number underflowed to a subnormal or zero.
fn strtod_erange(s: &[u8], v: f64) -> bool {
    if v.is_infinite() {
        return true;
    }
    if v == 0.0 || v.abs() < f64::MIN_POSITIVE {
        let mant = s.split(|&c| c == b'e').next().unwrap_or(&[]);
        return mant.iter().any(|&c| (b'1'..=b'9').contains(&c));
    }
    false
}

/// `get_real` (static).
fn get_real(data: &[u8], p: &mut usize, status: &mut i32) -> f64 {
    let end = data.len();
    let mut result = 0.0;
    let mut nibble: i32 = 0;
    let mut pos: i32;
    let mut fail = false;
    let mut work_buffer: Vec<u8> = Vec::new();

    if at(data, *p) != 30 || *p + 1 >= end {
        *status = CFF_ERROR_PARSE_ERROR;
        return 0.0;
    }

    *p += 1; // skip first byte (30)

    pos = 0;
    while !fail && work_buffer.len() < WORK_BUFFER_SIZE - 2 && *p < end {
        // get nibble
        if pos % 2 != 0 {
            nibble = i32::from(data[*p] & 0x0f);
            *p += 1;
        } else {
            nibble = i32::from((data[*p] >> 4) & 0x0f);
        }
        if (0x00..=0x09).contains(&nibble) {
            work_buffer.push(nibble as u8 + b'0');
        } else if nibble == 0x0a {
            // .
            work_buffer.push(b'.');
        } else if nibble == 0x0b || nibble == 0x0c {
            // E, E-
            work_buffer.push(b'e');
            if nibble == 0x0c {
                work_buffer.push(b'-');
            }
        } else if nibble == 0x0e {
            // `-'
            work_buffer.push(b'-');
        } else if nibble == 0x0d {
            // skip
        } else if nibble == 0x0f {
            // end
            if pos % 2 == 0 && at(data, *p) != 0xff {
                fail = true;
            }
            break;
        } else {
            // invalid
            fail = true;
        }
        pos += 1;
    }

    // returned values
    if fail || nibble != 0x0f {
        *status = CFF_ERROR_PARSE_ERROR;
    } else {
        let (v, n) = crate::fmt::strtod(&work_buffer);
        result = v;
        if n != work_buffer.len() || strtod_erange(&work_buffer, v) {
            *status = CFF_ERROR_PARSE_ERROR;
        }
    }

    result
}

/// `add_dict` (static): the operator at `data[*p]` with the stack's
/// operands.
fn add_dict(dict: &mut CffDict, st: &mut DictStack, data: &[u8], p: &mut usize, status: &mut i32) {
    let end = data.len();
    let mut id = data[*p] as usize;
    if id == 0x0c {
        *p += 1;
        if *p >= end || {
            id = data[*p] as usize + CFF_LAST_DICT_OP1;
            id >= CFF_LAST_DICT_OP
        } {
            *status = CFF_ERROR_PARSE_ERROR;
            return;
        }
    } else if id >= CFF_LAST_DICT_OP1 {
        *status = CFF_ERROR_PARSE_ERROR;
        return;
    }

    let (opname, argtype) = DICT_OPERATOR[id];
    let Some(opname) = opname.filter(|_| argtype >= 0) else {
        // YuppySC-Regular.otf from OS X for instance uses op id 37, simply
        // ignore this dict instead of treat it as parsing error.
        return;
    };

    if dict.count >= dict.max {
        dict.max += DICT_ENTRY_MAX;
    }

    if argtype == CFF_TYPE_NUMBER
        || argtype == CFF_TYPE_BOOLEAN
        || argtype == CFF_TYPE_SID
        || argtype == CFF_TYPE_OFFSET
    {
        // check for underflow here, as exactly one operand is expected
        if st.stack_top < 1 {
            *status = CFF_ERROR_STACK_UNDERFLOW;
            return;
        }
        st.stack_top -= 1;
        dict.entries.push(CffDictEntry {
            id: id as i32,
            key: opname,
            count: 1,
            values: vec![st.arg_stack[st.stack_top as usize]],
        });
        dict.count += 1;
    } else {
        // just ignore operator if there were no operands provided; don't
        // treat this as underflow (e.g. StemSnapV in
        // TemporaLGCUni-Italic.otf)
        if st.stack_top > 0 {
            let n = st.stack_top as usize;
            dict.entries.push(CffDictEntry {
                id: id as i32,
                key: opname,
                count: st.stack_top,
                values: st.arg_stack[..n].to_vec(),
            });
            st.stack_top = 0;
            dict.count += 1;
        }
    }

    *p += 1;
}

/// `cff_dict_unpack`: `data` ends at C's `endptr`; none on error.
pub fn cff_dict_unpack(data: &[u8]) -> Result<Option<CffDict>> {
    let mut status = CFF_PARSE_OK;
    let mut st = DictStack::default();
    let end = data.len();
    let mut p = 0usize;

    st.stack_top = 0;

    let mut dict = cff_new_dict();
    while p < end && status == CFF_PARSE_OK {
        let b = data[p];
        if b < 22 {
            // operator
            add_dict(&mut dict, &mut st, data, &mut p, &mut status);
        } else if b == 30 {
            // real - First byte of a sequence (variable)
            if (st.stack_top as usize) < CFF_DICT_STACK_LIMIT {
                st.arg_stack[st.stack_top as usize] = get_real(data, &mut p, &mut status);
                st.stack_top += 1;
            } else {
                status = CFF_ERROR_STACK_OVERFLOW;
            }
        } else if b == 255 || (22..=27).contains(&b) {
            // reserved
            p += 1;
        } else {
            // everything else are integer
            if (st.stack_top as usize) < CFF_DICT_STACK_LIMIT {
                st.arg_stack[st.stack_top as usize] = get_integer(data, &mut p, &mut status);
                st.stack_top += 1;
            } else {
                status = CFF_ERROR_STACK_OVERFLOW;
            }
        }
    }

    if status != CFF_PARSE_OK {
        fatal!("CFF: Parsing CFF DICT failed. (error={})", status);
    } else if st.stack_top != 0 {
        warn!("CFF: Garbage in CFF DICT data.");
        st.stack_top = 0;
    }

    Ok(Some(dict))
}

/// `pack_integer` (static): bytes written.
fn pack_integer(dest: &mut [u8], value: i32) -> Result<i32> {
    let destlen = dest.len();
    let mut value = value;
    let len;

    if (-107..=107).contains(&value) {
        if destlen < 1 {
            fatal!("CFF: Buffer overflow.");
        }
        dest[0] = ((value + 139) & 0xff) as u8;
        len = 1;
    } else if (108..=1131).contains(&value) {
        if destlen < 2 {
            fatal!("CFF: Buffer overflow.");
        }
        value = 0xf700 + value - 108;
        dest[0] = ((value >> 8) & 0xff) as u8;
        dest[1] = (value & 0xff) as u8;
        len = 2;
    } else if (-1131..=-108).contains(&value) {
        if destlen < 2 {
            fatal!("CFF: Buffer overflow.");
        }
        value = 0xfb00 - value - 108;
        dest[0] = ((value >> 8) & 0xff) as u8;
        dest[1] = (value & 0xff) as u8;
        len = 2;
    } else if (-32768..=32767).contains(&value) {
        // shortint
        if destlen < 3 {
            fatal!("CFF: Buffer overflow.");
        }
        dest[0] = 28;
        dest[1] = ((value >> 8) & 0xff) as u8;
        dest[2] = (value & 0xff) as u8;
        len = 3;
    } else {
        // longint
        if destlen < 5 {
            fatal!("CFF: Buffer overflow.");
        }
        dest[0] = 29;
        dest[1] = ((value >> 24) & 0xff) as u8;
        dest[2] = ((value >> 16) & 0xff) as u8;
        dest[3] = ((value >> 8) & 0xff) as u8;
        dest[4] = (value & 0xff) as u8;
        len = 5;
    }

    Ok(len)
}

/// C's `sprintf("%.*g", prec, value)` for a finite `value`.
pub fn sprintf_g(value: f64, prec: usize) -> Vec<u8> {
    use core::fmt::Write;
    let p = if prec == 0 { 1 } else { prec };
    let mut out = crate::fmt::Buf::new();
    if !value.is_finite() {
        let _ = write!(
            out,
            "{}",
            if value.is_nan() {
                if value.is_sign_negative() {
                    "-nan"
                } else {
                    "nan"
                }
            } else if value < 0.0 {
                "-inf"
            } else {
                "inf"
            }
        );
        return out.0;
    }
    // The exponent %e gives, after rounding to `p` digits.
    let mut e = crate::fmt::Buf::new();
    let _ = write!(e, "{:.*e}", p - 1, value);
    let s = e.0;
    let epos = s.iter().position(|&c| c == b'e').unwrap();
    let x: i32 = core::str::from_utf8(&s[epos + 1..])
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or(0);
    let strip = |v: &mut Vec<u8>| {
        if v.contains(&b'.') {
            while v.last() == Some(&b'0') {
                v.pop();
            }
            if v.last() == Some(&b'.') {
                v.pop();
            }
        }
    };
    if x < -4 || x >= p as i32 {
        let mut m = s[..epos].to_vec();
        strip(&mut m);
        out.extend(&m);
        let _ = write!(
            out,
            "e{}{:02}",
            if x < 0 { '-' } else { '+' },
            x.unsigned_abs()
        );
    } else {
        let _ = write!(out, "{:.*}", (p as i32 - 1 - x) as usize, value);
        strip(&mut out.0);
    }
    out.0
}

/// `pack_real` (static): bytes written.
fn pack_real(dest: &mut [u8], value: f64) -> Result<i32> {
    let destlen = dest.len() as i32;
    let mut value = value;
    let mut pos: i32 = 2;

    if destlen < 2 {
        fatal!("CFF: Buffer overflow.");
    }

    dest[0] = 30;

    if value == 0.0 {
        dest[1] = 0x0f;
        return Ok(2);
    }

    if value < 0.0 {
        dest[1] = 0xe0;
        value *= -1.0;
        pos += 1;
    }

    // To avoid the problem with Mac OS X 10.4 Quartz, change the presion
    // of the real numbers on June 27, 2007 for musix20.pfb
    let buffer = sprintf_g(value, 13);

    let mut i = 0usize;
    while i < buffer.len() {
        let ch: u8;
        if buffer[i] == b'.' {
            ch = 0x0a;
        } else if buffer[i].is_ascii_digit() {
            ch = buffer[i] - b'0';
        } else if buffer[i] == b'e' {
            i += 1;
            ch = if buffer.get(i) == Some(&b'-') {
                0x0c
            } else {
                0x0b
            };
        } else {
            fatal!("CFF: Invalid character.");
        }

        if destlen < pos / 2 + 1 {
            fatal!("CFF: Buffer overflow.");
        }

        if pos % 2 != 0 {
            dest[(pos / 2) as usize] = dest[(pos / 2) as usize].wrapping_add(ch);
        } else {
            dest[(pos / 2) as usize] = ch << 4;
        }
        pos += 1;
        i += 1;
    }

    if pos % 2 != 0 {
        dest[(pos / 2) as usize] = dest[(pos / 2) as usize].wrapping_add(0x0f);
        pos += 1;
    } else {
        if destlen < pos / 2 + 1 {
            fatal!("CFF: Buffer overflow.");
        }
        dest[(pos / 2) as usize] = 0xff;
        pos += 2;
    }

    Ok(pos / 2)
}

/// `cff_dict_put_number` (static): bytes written.
fn cff_dict_put_number(value: f64, dest: &mut [u8], ty: i32) -> Result<i32> {
    let len;
    let nearint = libm::floor(value + 0.5);
    // set offset to longint
    if ty == CFF_TYPE_OFFSET {
        let lvalue = value as i32;
        if dest.len() < 5 {
            fatal!("CFF: Buffer overflow.");
        }
        dest[0] = 29;
        dest[1] = ((lvalue >> 24) & 0xff) as u8;
        dest[2] = ((lvalue >> 16) & 0xff) as u8;
        dest[3] = ((lvalue >> 8) & 0xff) as u8;
        dest[4] = (lvalue & 0xff) as u8;
        len = 5;
    } else if value > f64::from(CFF_INT_MAX)
        || value < f64::from(CFF_INT_MIN)
        || libm::fabs(value - nearint) > 1.0e-5
    {
        // real
        len = pack_real(dest, value)?;
    } else {
        // integer
        len = pack_integer(dest, nearint as i32)?;
    }

    Ok(len)
}

/// `put_dict_entry` (static): bytes written.
fn put_dict_entry(de: &CffDictEntry, dest: &mut [u8]) -> Result<i32> {
    let destlen = dest.len() as i32;
    let mut len: i32 = 0;

    if de.count > 0 {
        let id = de.id;
        let argtype = DICT_OPERATOR[id as usize].1;
        let ty = if argtype == CFF_TYPE_OFFSET || argtype == CFF_TYPE_SZOFF {
            CFF_TYPE_OFFSET
        } else {
            CFF_TYPE_NUMBER
        };
        for i in 0..de.count as usize {
            len += cff_dict_put_number(de.values[i], &mut dest[len as usize..], ty)?;
        }
        if id >= 0 && (id as usize) < CFF_LAST_DICT_OP1 {
            if len + 1 > destlen {
                fatal!("CFF: Buffer overflow.");
            }
            dest[len as usize] = id as u8;
            len += 1;
        } else if id >= 0 && (id as usize) < CFF_LAST_DICT_OP {
            if len + 2 > destlen {
                fatal!("in cff_dict_pack(): Buffer overflow");
            }
            dest[len as usize] = 12;
            dest[len as usize + 1] = (id as usize - CFF_LAST_DICT_OP1) as u8;
            len += 2;
        } else {
            fatal!("CFF: Invalid CFF DICT operator ID.");
        }
    }

    Ok(len)
}

/// `dict_operator`'s id of `key`, as `cff_dict_add` looks it up.
fn op_id(key: &[u8]) -> usize {
    let mut id = 0;
    while id < CFF_LAST_DICT_OP {
        if DICT_OPERATOR[id].0 == Some(key) {
            break;
        }
        id += 1;
    }
    id
}

impl CffDict {
    /// `cff_dict_pack`: bytes written.
    pub fn cff_dict_pack(&self, dest: &mut [u8]) -> Result<i32> {
        let mut len: i32 = 0;

        for i in 0..self.count as usize {
            if self.entries[i].key == b"ROS" {
                len += put_dict_entry(&self.entries[i], dest)?;
                break;
            }
        }
        for i in 0..self.count as usize {
            if self.entries[i].key != b"ROS" {
                len += put_dict_entry(&self.entries[i], &mut dest[len as usize..])?;
            }
        }

        Ok(len)
    }
    /// `cff_dict_add`.
    pub fn cff_dict_add(&mut self, key: &[u8], count: i32) -> Result<()> {
        let id = op_id(key);

        if id == CFF_LAST_DICT_OP {
            fatal!("CFF: Unknown CFF DICT operator.");
        }

        for i in 0..self.count as usize {
            if self.entries[i].id == id as i32 {
                if self.entries[i].count != count {
                    fatal!("CFF: Inconsistent DICT argument number.");
                }
                return Ok(());
            }
        }

        if self.count + 1 >= self.max {
            self.max += 8;
        }

        self.entries.push(CffDictEntry {
            id: id as i32,
            key: DICT_OPERATOR[id].0.unwrap(),
            count,
            values: if count > 0 {
                vec![0.0; count as usize]
            } else {
                Vec::new()
            },
        });
        self.count += 1;
        Ok(())
    }
    /// `cff_dict_remove`.
    pub fn cff_dict_remove(&mut self, key: &[u8]) {
        for i in 0..self.count as usize {
            if key == self.entries[i].key {
                self.entries[i].count = 0;
                self.entries[i].values = Vec::new();
            }
        }
    }
    /// `cff_dict_known`.
    pub fn cff_dict_known(&self, key: &[u8]) -> i32 {
        for i in 0..self.count as usize {
            if key == self.entries[i].key && self.entries[i].count > 0 {
                return 1;
            }
        }
        0
    }
    /// `cff_dict_get`.
    pub fn cff_dict_get(&self, key: &[u8], idx: i32) -> Result<f64> {
        let mut value = 0.0;
        let mut i = 0usize;
        while i < self.count as usize {
            if key == self.entries[i].key {
                if self.entries[i].count > idx {
                    value = self.entries[i].values[idx as usize];
                } else {
                    fatal!("CFF: Invalid index number.");
                }
                break;
            }
            i += 1;
        }

        if i == self.count as usize {
            fatal!(
                "CFF: DICT entry \"{}\" not found.",
                core::str::from_utf8(key).unwrap_or("?")
            );
        }

        Ok(value)
    }
    /// `cff_dict_set`.
    pub fn cff_dict_set(&mut self, key: &[u8], idx: i32, value: f64) -> Result<()> {
        let mut i = 0usize;
        while i < self.count as usize {
            if key == self.entries[i].key {
                if self.entries[i].count > idx {
                    self.entries[i].values[idx as usize] = value;
                } else {
                    fatal!("CFF: Invalid index number.");
                }
                break;
            }
            i += 1;
        }

        if i == self.count as usize {
            fatal!(
                "CFF: DICT entry \"{}\" not found.",
                core::str::from_utf8(key).unwrap_or("?")
            );
        }
        Ok(())
    }
}

/// `cff_dict_update`: SIDs re-added to `cff`'s `_string` (see the
/// module doc for a dict of `cff` itself).
pub fn cff_dict_update(dict: &mut CffDict, cff: &mut CffFont) {
    for i in 0..dict.count as usize {
        if dict.entries[i].count > 0 {
            let id = dict.entries[i].id;
            let argtype = DICT_OPERATOR[id as usize].1;
            if argtype == CFF_TYPE_SID {
                let s = cff.cff_get_string(dict.entries[i].values[0] as SSid);
                dict.entries[i].values[0] = f64::from(cff.cff_add_string(&s, 1));
            } else if argtype == CFF_TYPE_ROS {
                let s = cff.cff_get_string(dict.entries[i].values[0] as SSid);
                dict.entries[i].values[0] = f64::from(cff.cff_add_string(&s, 1));
                let s = cff.cff_get_string(dict.entries[i].values[1] as SSid);
                dict.entries[i].values[1] = f64::from(cff.cff_add_string(&s, 1));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packr(v: f64) -> Vec<u8> {
        let mut d = [0u8; 32];
        let n = pack_real(&mut d, v).unwrap();
        d[..n as usize].to_vec()
    }

    #[test]
    fn g13() {
        assert_eq!(sprintf_g(0.001, 13), b"0.001");
        assert_eq!(sprintf_g(1e-5, 13), b"1e-05");
        assert_eq!(sprintf_g(0.1, 13), b"0.1");
        assert_eq!(sprintf_g(1.0 / 3.0, 13), b"0.3333333333333");
        assert_eq!(sprintf_g(123456.5, 13), b"123456.5");
        assert_eq!(sprintf_g(1e13, 13), b"1e+13");
        assert_eq!(sprintf_g(1e12, 13), b"1000000000000");
        assert_eq!(sprintf_g(0.039625, 13), b"0.039625");
        assert_eq!(sprintf_g(2.5, 0), b"2");
    }

    #[test]
    fn reals() {
        assert_eq!(packr(0.0), [30, 0x0f]);
        // -2.25: e2 a2 5f
        assert_eq!(packr(-2.25), [30, 0xe2, 0xa2, 0x5f]);
        // 0.001: 0a 00 1f
        assert_eq!(packr(0.001), [30, 0x0a, 0x00, 0x1f]);
        // 1e-05: 1c 05 ff
        assert_eq!(packr(1e-5), [30, 0x1c, 0x05, 0xff]);
        // 0.5: 0a 5f
        assert_eq!(packr(0.5), [30, 0x0a, 0x5f]);
    }

    #[test]
    fn integers_and_unpack() {
        let mut d = [0u8; 8];
        assert_eq!(pack_integer(&mut d, 0).unwrap(), 1);
        assert_eq!(d[0], 139);
        assert_eq!(pack_integer(&mut d, 1000).unwrap(), 2);
        assert_eq!(&d[..2], &[0xfa, 0x7c]);
        assert_eq!(pack_integer(&mut d, -1000).unwrap(), 2);
        assert_eq!(&d[..2], &[0xfe, 0x7c]);
        assert_eq!(pack_integer(&mut d, 5000).unwrap(), 3);
        assert_eq!(&d[..3], &[28, 0x13, 0x88]);
        assert_eq!(pack_integer(&mut d, 100000).unwrap(), 5);
        assert_eq!(&d[..5], &[29, 0, 1, 0x86, 0xa0]);

        // FontBBox [-50 -100 1000 900], ItalicAngle -2.25, Private 20 300
        let src = [
            89, 39, 0xfa, 0x7c, 0xfa, 0x18, 5, 30, 0xe2, 0xa2, 0x5f, 12, 2, 159, 0xf7, 0xc0, 18,
        ];
        let dict = cff_dict_unpack(&src).unwrap().unwrap();
        assert_eq!(dict.count, 3);
        assert_eq!(dict.cff_dict_get(b"FontBBox", 0).unwrap(), -50.0);
        assert_eq!(dict.cff_dict_get(b"FontBBox", 1).unwrap(), -100.0);
        assert_eq!(dict.cff_dict_get(b"FontBBox", 2).unwrap(), 1000.0);
        assert_eq!(dict.cff_dict_get(b"FontBBox", 3).unwrap(), 900.0);
        assert_eq!(dict.cff_dict_get(b"ItalicAngle", 0).unwrap(), -2.25);
        assert_eq!(dict.cff_dict_get(b"Private", 1).unwrap(), 300.0);
        let mut out = [0u8; 64];
        let n = dict.cff_dict_pack(&mut out).unwrap() as usize;
        // Private is packed as two longints.
        let mut want = src[..13].to_vec();
        want.extend_from_slice(&[29, 0, 0, 0, 20, 29, 0, 0, 1, 44, 18]);
        assert_eq!(&out[..n], &want[..]);
    }
}
