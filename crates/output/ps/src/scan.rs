//! The scanner: PostScript's syntax (PLRM 3.2) as Ghostscript reads it
//! (iscan.c): numbers (radix numbers as 32-bit, integers out of 32 bits
//! made reals), strings with their escapes, hexadecimal and base-85
//! strings, names, procedures.

use alloc::vec::Vec;

use crate::file::File;

/// A token.
pub enum Tok {
    Int(i64),
    Real(f32),
    /// A string's bytes.
    Str(Vec<u8>),
    /// `/name`.
    LitName(Vec<u8>),
    /// `//name`.
    ImmName(Vec<u8>),
    /// An executable name.
    Name(Vec<u8>),
    /// `{`.
    ProcStart,
    /// `}`.
    ProcEnd,
    /// A DSC comment (`%%...` at a line's start) of a file that asks for
    /// them ([`File::dsc`]): the line.
    Dsc(Vec<u8>),
}

pub enum ScanErr {
    Syntax,
}

pub fn is_ws(c: u8) -> bool {
    matches!(c, 0 | 9 | 10 | 12 | 13 | 32)
}

pub fn is_delim(c: u8) -> bool {
    matches!(
        c,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

/// The next token of `f`, `None` at its end.
pub fn token(f: &mut File) -> Result<Option<Tok>, ScanErr> {
    loop {
        let Some(c) = f.next() else {
            return Ok(None);
        };
        if is_ws(c) {
            continue;
        }
        match c {
            b'%' => {
                let at_line_start =
                    f.pos < 2 || matches!(f.data.get(f.pos - 2), Some(b'\n' | b'\r'));
                let start = f.pos - 1;
                while let Some(c) = f.next() {
                    if c == b'\n' || c == b'\r' || c == 0x0c {
                        break;
                    }
                }
                if f.dsc && at_line_start && f.data.get(start + 1) == Some(&b'%') {
                    let mut end = f.pos;
                    while end > start && matches!(f.data[end - 1], b'\n' | b'\r' | 0x0c) {
                        end -= 1;
                    }
                    return Ok(Some(Tok::Dsc(f.data[start..end].to_vec())));
                }
            }
            b'(' => return string(f).map(|s| Some(Tok::Str(s))),
            b'<' => {
                if f.peek() == Some(b'<') {
                    f.next();
                    return Ok(Some(Tok::Name(b"<<".to_vec())));
                }
                if f.peek() == Some(b'~') {
                    f.next();
                    return a85(f).map(|s| Some(Tok::Str(s)));
                }
                return hex(f).map(|s| Some(Tok::Str(s)));
            }
            b'>' => {
                if f.peek() == Some(b'>') {
                    f.next();
                    return Ok(Some(Tok::Name(b">>".to_vec())));
                }
                return Err(ScanErr::Syntax);
            }
            b'[' | b']' => return Ok(Some(Tok::Name(alloc::vec![c]))),
            b'{' => return Ok(Some(Tok::ProcStart)),
            b'}' => return Ok(Some(Tok::ProcEnd)),
            b')' => return Err(ScanErr::Syntax),
            b'/' => {
                let imm = f.peek() == Some(b'/');
                if imm {
                    f.next();
                }
                let n = word(f, None);
                return Ok(Some(if imm {
                    Tok::ImmName(n)
                } else {
                    Tok::LitName(n)
                }));
            }
            _ => {
                let w = word(f, Some(c));
                if let Some(t) = number(&w) {
                    return Ok(Some(t));
                }
                return Ok(Some(Tok::Name(w)));
            }
        }
    }
}

/// The characters of a name or number, up to a delimiter or white space
/// (the white space after it is read, the delimiter is not).
fn word(f: &mut File, first: Option<u8>) -> Vec<u8> {
    let mut w = Vec::new();
    w.extend(first);
    while let Some(c) = f.peek() {
        if is_ws(c) {
            f.next();
            // (a CR LF after a token is read whole)
            if c == b'\r' && f.peek() == Some(b'\n') {
                f.next();
            }
            break;
        }
        if is_delim(c) {
            break;
        }
        w.push(c);
        f.next();
    }
    w
}

fn string(f: &mut File) -> Result<Vec<u8>, ScanErr> {
    let mut s = Vec::new();
    let mut depth = 0;
    loop {
        let Some(c) = f.next() else {
            return Err(ScanErr::Syntax);
        };
        match c {
            b'(' => {
                depth += 1;
                s.push(c);
            }
            b')' => {
                if depth == 0 {
                    return Ok(s);
                }
                depth -= 1;
                s.push(c);
            }
            b'\r' => {
                if f.peek() == Some(b'\n') {
                    f.next();
                }
                s.push(b'\n');
            }
            b'\\' => {
                let Some(e) = f.next() else {
                    return Err(ScanErr::Syntax);
                };
                match e {
                    b'n' => s.push(b'\n'),
                    b'r' => s.push(b'\r'),
                    b't' => s.push(b'\t'),
                    b'b' => s.push(8),
                    b'f' => s.push(12),
                    b'\n' => {}
                    b'\r' => {
                        if f.peek() == Some(b'\n') {
                            f.next();
                        }
                    }
                    b'0'..=b'7' => {
                        let mut v = u32::from(e - b'0');
                        for _ in 0..2 {
                            match f.peek() {
                                Some(d @ b'0'..=b'7') => {
                                    f.next();
                                    v = v * 8 + u32::from(d - b'0');
                                }
                                _ => break,
                            }
                        }
                        s.push(v as u8);
                    }
                    _ => s.push(e),
                }
            }
            _ => s.push(c),
        }
    }
}

fn hex(f: &mut File) -> Result<Vec<u8>, ScanErr> {
    let mut s = Vec::new();
    let mut hi: Option<u8> = None;
    loop {
        let Some(c) = f.next() else {
            return Err(ScanErr::Syntax);
        };
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            b'>' => break,
            _ if is_ws(c) => continue,
            _ => return Err(ScanErr::Syntax),
        };
        match hi.take() {
            None => hi = Some(d),
            Some(h) => s.push(h << 4 | d),
        }
    }
    if let Some(h) = hi {
        s.push(h << 4);
    }
    Ok(s)
}

fn a85(f: &mut File) -> Result<Vec<u8>, ScanErr> {
    let start = f.pos;
    let rest = &f.data[start..];
    let (out, used) = crate::file::decode(
        crate::file::Filter::Ascii85,
        rest,
        &crate::file::FilterParams::default(),
    );
    f.pos = start + used;
    Ok(out)
}

/// A number token, if `w` is one.
pub fn number(w: &[u8]) -> Option<Tok> {
    if w.is_empty() {
        return None;
    }
    // (radix: base#digits)
    if let Some(h) = w.iter().position(|&c| c == b'#') {
        let base: u32 = core::str::from_utf8(&w[..h]).ok()?.parse().ok()?;
        if !(2..=36).contains(&base) || h == 0 || !w[..h].iter().all(u8::is_ascii_digit) {
            return None;
        }
        let digits = &w[h + 1..];
        if digits.is_empty() {
            return None;
        }
        let mut v: u64 = 0;
        for &c in digits {
            let d = (c as char).to_digit(36)?;
            if d >= base {
                return None;
            }
            v = v.checked_mul(u64::from(base))?.checked_add(u64::from(d))?;
            if v > u64::from(u32::MAX) {
                // (too big for 32 bits: a real)
                return Some(Tok::Real(v as f32));
            }
        }
        return Some(Tok::Int(i64::from(v as u32 as i32)));
    }
    let mut i = 0;
    if w[0] == b'+' || w[0] == b'-' {
        i = 1;
    }
    let digits_start = i;
    while i < w.len() && w[i].is_ascii_digit() {
        i += 1;
    }
    let int_digits = i - digits_start;
    if i == w.len() {
        if int_digits == 0 {
            return None;
        }
        let s = core::str::from_utf8(w).ok()?;
        return match s.parse::<i64>() {
            Ok(v) if i32::try_from(v).is_ok() => Some(Tok::Int(v)),
            _ => Some(Tok::Real(s.parse::<f64>().ok()? as f32)),
        };
    }
    let mut frac_digits = 0;
    if w[i] == b'.' {
        i += 1;
        let s = i;
        while i < w.len() && w[i].is_ascii_digit() {
            i += 1;
        }
        frac_digits = i - s;
    }
    if int_digits == 0 && frac_digits == 0 {
        return None;
    }
    if i < w.len() && (w[i] == b'e' || w[i] == b'E') {
        i += 1;
        if i < w.len() && (w[i] == b'+' || w[i] == b'-') {
            i += 1;
        }
        let s = i;
        while i < w.len() && w[i].is_ascii_digit() {
            i += 1;
        }
        if i == s {
            return None;
        }
    }
    if i != w.len() {
        return None;
    }
    let s = core::str::from_utf8(w).ok()?;
    let s = s.strip_prefix('+').unwrap_or(s);
    let s = if s.ends_with('.') {
        alloc::format!("{s}0")
    } else {
        alloc::string::String::from(s)
    };
    let s = if s.starts_with('.') {
        alloc::format!("0{s}")
    } else if s.starts_with("-.") {
        alloc::format!("-0{}", &s[1..])
    } else {
        s
    };
    Some(Tok::Real(s.parse::<f64>().ok()? as f32))
}
