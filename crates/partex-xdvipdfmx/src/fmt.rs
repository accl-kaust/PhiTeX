//! Number formatting as dvipdfm-x does it (`p_itoa`, `p_dtoa`, the
//! `sprintf` formats it uses), into byte buffers.

use alloc::vec::Vec;
use core::fmt::Write;

/// A byte buffer that `core::fmt` can write into.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Buf(pub Vec<u8>);

impl Buf {
    #[must_use]
    pub fn new() -> Self {
        Buf(Vec::new())
    }
    pub fn push(&mut self, c: u8) {
        self.0.push(c);
    }
    pub fn extend(&mut self, s: &[u8]) {
        self.0.extend_from_slice(s);
    }
    /// `%d`.
    pub fn int(&mut self, v: i32) {
        let _ = write!(self, "{v}");
    }
    /// `%ld` and the like.
    pub fn long(&mut self, v: i64) {
        let _ = write!(self, "{v}");
    }
    /// `%u`.
    pub fn uint(&mut self, v: u32) {
        let _ = write!(self, "{v}");
    }
    /// `%0Nu`.
    pub fn zero_padded(&mut self, v: u64, width: usize) {
        let _ = write!(self, "{v:0width$}");
    }
    /// `%.Nf`.
    pub fn fixed(&mut self, v: f64, prec: usize) {
        let _ = write!(self, "{v:.prec$}");
    }
    /// `p_dtoa`.
    pub fn dtoa(&mut self, v: f64, prec: usize) -> usize {
        let start = self.0.len();
        p_dtoa(self, v, prec);
        self.0.len() - start
    }
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl Write for Buf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.0.extend_from_slice(s.as_bytes());
        Ok(())
    }
}

const P10: [i32; 10] = [
    1,
    10,
    100,
    1000,
    10000,
    100_000,
    1_000_000,
    10_000_000,
    100_000_000,
    1_000_000_000,
];

/// `p_dtoa` (pdfdev.c): at most `prec` decimals, trailing zeros cut,
/// `0` for zero. A negative number that rounds to zero prints `0`, and
/// one between -1 and 0 prints without its leading zero (`-.5`).
pub fn p_dtoa(b: &mut Buf, value: f64, prec: usize) {
    let start = b.0.len();
    let mut value = value;
    if value < 0.0 {
        value = -value;
        b.push(b'-');
    }
    let mut i = libm::trunc(value);
    let f = value - i;
    let mut g = (f * f64::from(P10[prec]) + 0.5) as i32;
    if g == P10[prec] {
        g = 0;
        i += 1.0;
    }
    if i != 0.0 {
        let _ = write!(b, "{i:.0}");
    } else if g == 0 {
        b.0.truncate(start);
        b.push(b'0');
    }
    if g != 0 {
        b.push(b'.');
        let at = b.0.len();
        b.0.resize(at + prec, b'0');
        let mut j = prec;
        while j > 0 {
            j -= 1;
            b.0[at + j] = b'0' + (g % 10) as u8;
            g /= 10;
        }
        while b.0.last() == Some(&b'0') {
            b.0.pop();
        }
    }
}

/// `p_itoa`.
pub fn p_itoa(b: &mut Buf, v: i32) {
    b.int(v);
}

/// `pdf_sprint_number`: `p_dtoa` at 8 decimals.
pub fn sprint_number(b: &mut Buf, v: f64) {
    p_dtoa(b, v, 8);
}

/// `ROUND(n, acc)`: `floor(n/acc + 0.5)*acc`.
#[must_use]
pub fn round_acc(n: f64, acc: f64) -> f64 {
    libm::floor(n / acc + 0.5) * acc
}

/// C's `atof`/`strtod` on a byte string (the prefix that is a number).
#[must_use]
pub fn atof(s: &[u8]) -> f64 {
    let s = trim_c_space(s);
    let mut end = 0;
    let n = s.len();
    if end < n && (s[end] == b'+' || s[end] == b'-') {
        end += 1;
    }
    let digits_start = end;
    while end < n && s[end].is_ascii_digit() {
        end += 1;
    }
    if end < n && s[end] == b'.' {
        end += 1;
        while end < n && s[end].is_ascii_digit() {
            end += 1;
        }
    }
    if end == digits_start || (end == digits_start + 1 && s[digits_start] == b'.') {
        return 0.0;
    }
    if end < n && (s[end] == b'e' || s[end] == b'E') {
        let mut e = end + 1;
        if e < n && (s[e] == b'+' || s[e] == b'-') {
            e += 1;
        }
        if e < n && s[e].is_ascii_digit() {
            while e < n && s[e].is_ascii_digit() {
                e += 1;
            }
            end = e;
        }
    }
    core::str::from_utf8(&s[..end])
        .ok()
        .and_then(|t| t.parse::<f64>().ok())
        .unwrap_or(0.0)
}

/// C's `atoi`/`strtol(…, 10)`: the leading integer, 0 if none.
#[must_use]
pub fn atoi(s: &[u8]) -> i64 {
    let s = trim_c_space(s);
    let mut i = 0;
    let mut neg = false;
    if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        neg = s[i] == b'-';
        i += 1;
    }
    let mut v: i64 = 0;
    while i < s.len() && s[i].is_ascii_digit() {
        v = v.wrapping_mul(10).wrapping_add(i64::from(s[i] - b'0'));
        i += 1;
    }
    if neg { -v } else { v }
}

/// `strtol(s, &end, base)`: the value and how many bytes it took (0 if
/// none).
#[must_use]
pub fn strtol(s: &[u8], base: u32) -> (i64, usize) {
    let mut i = 0;
    while i < s.len() && is_c_space(s[i]) {
        i += 1;
    }
    let mut neg = false;
    if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        neg = s[i] == b'-';
        i += 1;
    }
    let mut base = base;
    if (base == 0 || base == 16)
        && i + 1 < s.len()
        && s[i] == b'0'
        && (s[i + 1] == b'x' || s[i + 1] == b'X')
        && s.get(i + 2).is_some_and(u8::is_ascii_hexdigit)
    {
        i += 2;
        base = 16;
    } else if base == 0 {
        base = if s.get(i) == Some(&b'0') { 8 } else { 10 };
    }
    let start = i;
    let mut v: i64 = 0;
    while i < s.len() {
        let d = match s[i] {
            c @ b'0'..=b'9' => u32::from(c - b'0'),
            c @ b'a'..=b'z' => u32::from(c - b'a') + 10,
            c @ b'A'..=b'Z' => u32::from(c - b'A') + 10,
            _ => break,
        };
        if d >= base {
            break;
        }
        v = v
            .saturating_mul(i64::from(base))
            .saturating_add(i64::from(d));
        i += 1;
    }
    if i == start {
        return (0, 0);
    }
    (if neg { -v } else { v }, i)
}

/// `strtod(s, &end)`: the value and how many bytes it took.
#[must_use]
pub fn strtod(s: &[u8]) -> (f64, usize) {
    let mut i = 0;
    while i < s.len() && is_c_space(s[i]) {
        i += 1;
    }
    let start = i;
    let n = s.len();
    let mut e = i;
    if e < n && (s[e] == b'+' || s[e] == b'-') {
        e += 1;
    }
    let ds = e;
    while e < n && s[e].is_ascii_digit() {
        e += 1;
    }
    let mut nd = e - ds;
    if e < n && s[e] == b'.' {
        let f = e + 1;
        let mut g = f;
        while g < n && s[g].is_ascii_digit() {
            g += 1;
        }
        nd += g - f;
        if nd > 0 {
            e = g;
        }
    }
    if nd == 0 {
        return (0.0, 0);
    }
    if e < n && (s[e] == b'e' || s[e] == b'E') {
        let mut x = e + 1;
        if x < n && (s[x] == b'+' || s[x] == b'-') {
            x += 1;
        }
        if x < n && s[x].is_ascii_digit() {
            while x < n && s[x].is_ascii_digit() {
                x += 1;
            }
            e = x;
        }
    }
    let text = &s[start..e];
    let v = core::str::from_utf8(text)
        .ok()
        .and_then(|t| {
            let t = t.strip_suffix('.').unwrap_or(t);
            t.parse::<f64>().ok()
        })
        .unwrap_or(0.0);
    (v, e)
}

/// C's `isspace`.
#[must_use]
pub fn is_c_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

fn trim_c_space(s: &[u8]) -> &[u8] {
    let mut i = 0;
    while i < s.len() && is_c_space(s[i]) {
        i += 1;
    }
    &s[i..]
}

/// `10^k` correctly rounded (glibc's `pow(10, k)`).
#[must_use]
pub fn pow10(k: i32) -> f64 {
    const EXACT: [f64; 23] = [
        1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16,
        1e17, 1e18, 1e19, 1e20, 1e21, 1e22,
    ];
    if (0..23).contains(&k) {
        return EXACT[k as usize];
    }
    let mut b = Buf::new();
    let _ = write!(b, "1e{k}");
    core::str::from_utf8(&b.0)
        .ok()
        .and_then(|t| t.parse::<f64>().ok())
        .unwrap_or(0.0)
}

impl Buf {
    /// `%0Nx`: `v` in lowercase hex, zero-padded to `width`.
    pub fn hex_padded(&mut self, v: u32, width: usize) {
        let mut digits = Vec::new();
        let mut v = v;
        loop {
            let d = (v & 0xf) as u8;
            digits.push(if d < 10 { b'0' + d } else { b'a' + d - 10 });
            v >>= 4;
            if v == 0 {
                break;
            }
        }
        for _ in digits.len()..width {
            self.push(b'0');
        }
        digits.reverse();
        self.extend(&digits);
    }
}
