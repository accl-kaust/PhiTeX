//! pdfTeX's arithmetic for PDF output (pdfTeX §687–§690): numbers are
//! divided into the raster of the decimal digits they are printed with,
//! and the rest of the division (`scaled_out`) is what positions advance
//! by.

use phitex_typeset::arith::Scaled;

/// `one_bp` (pdfTeX §688): 65781.76sp.
pub const ONE_BP: Scaled = 65782;
/// `one_hundred_bp` (pdfTeX §688).
pub const ONE_HUNDRED_BP: Scaled = 6_578_176;
/// `one_hundred_inch` (pdfTeX §688).
pub const ONE_HUNDRED_INCH: Scaled = 473_628_672;
/// `ten_pow` (pdfTeX §688).
pub const TEN_POW: [i64; 10] = [
    1,
    10,
    100,
    1_000,
    10_000,
    100_000,
    1_000_000,
    10_000_000,
    100_000_000,
    1_000_000_000,
];
/// `max_integer` (pdfTeX §687).
const MAX_INTEGER: i64 = 0x7FFF_FFFF;

/// `divide_scaled(s, m, dd)` (pdfTeX §689): `s/m` times `10^dd`, rounded,
/// and `scaled_out`, the part of `s` that this quotient stands for.
///
/// # Panics
/// When `m` is zero or too big ("pdfTeX error (arithmetic)", fatal).
#[must_use]
pub fn divide_scaled(s: Scaled, m: Scaled, dd: i32) -> (i32, Scaled) {
    let (mut s, mut m) = (i64::from(s), i64::from(m));
    let mut sign = 1;
    if s < 0 {
        sign = -sign;
        s = -s;
    }
    if m < 0 {
        sign = -sign;
        m = -m;
    }
    assert!(m != 0, "pdfTeX error (arithmetic): divided by zero");
    assert!(
        m < MAX_INTEGER / 10,
        "pdfTeX error (arithmetic): number too big"
    );
    // pdfTeX's digit loop (`q:=10*q+(10*r) div m; r:=(10*r) mod m`,
    // `dd` times after `q:=s div m; r:=s mod m`) keeps
    // `s*10^k = q*m + r, 0 <= r < m`, so it ends with the quotient and
    // remainder of `s*10^dd` by `m`: one division (`s < 2^31`, `dd <= 9`:
    // `s*10^dd < 2^31*10^9 < 2^63`)
    let (mut q, mut r) = match usize::try_from(dd) {
        Ok(k) if k < TEN_POW.len() => {
            let p = s * TEN_POW[k];
            (p / m, p % m)
        }
        Ok(_) => {
            let (mut q, mut r) = (s / m, s % m);
            for _ in 0..dd {
                q = 10 * q + (10 * r) / m;
                r = (10 * r) % m;
            }
            (q, r)
        }
        Err(_) => (s / m, s % m),
    };
    if 2 * r >= m {
        q += 1;
        r -= m;
    }
    let dd = usize::try_from(dd).unwrap_or(0);
    let out = sign * (s - r / TEN_POW[dd]);
    (narrow(sign * q), narrow(out))
}

/// `round_xn_over_d(x, n, d)` (pdfTeX §689): `x*n/d` rounded.
#[must_use]
pub fn round_xn_over_d(x: Scaled, n: i32, d: i32) -> Scaled {
    let positive = x >= 0;
    let x = i64::from(x).abs();
    let (n, d) = (i64::from(n), i64::from(d));
    let t = (x % 0o100000) * n;
    let mut u = (x / 0o100000) * n + (t / 0o100000);
    let mut v = (u % d) * 0o100000 + (t % 0o100000);
    if u / d < 0o100000 {
        u = 0o100000 * (u / d) + (v / d);
    }
    v %= d;
    if 2 * v >= d {
        u += 1;
    }
    narrow(if positive { u } else { -u })
}

/// An `i64` back into TeX's 32-bit integers (web2c's `integer`).
#[allow(clippy::cast_possible_truncation)]
fn narrow(x: i64) -> i32 {
    x as i32
}

/// `pdf_print_int` (pdfTeX §686) as bytes.
pub fn int(out: &mut Vec<u8>, n: i64) {
    // (below 1000, the common case (a kern in a `TJ` string), the digits
    // pushed as they are)
    let m = n.unsigned_abs();
    if m < 1000 {
        let m = u16::try_from(m).unwrap_or(0);
        if n < 0 {
            out.push(b'-');
        }
        let d = |x: u16| b'0' + u8::try_from(x % 10).unwrap_or(0);
        if m >= 100 {
            out.extend_from_slice(&[d(m / 100), d(m / 10), d(m)]);
        } else if m >= 10 {
            out.extend_from_slice(&[d(m / 10), d(m)]);
        } else {
            out.push(d(m));
        }
        return;
    }
    // (the digits on the stack, last first: no string per number)
    let mut buf = [0u8; 20];
    let mut k = buf.len();
    let mut m = n.unsigned_abs();
    loop {
        k -= 1;
        buf[k] = b'0' + u8::try_from(m % 10).unwrap_or(0);
        m /= 10;
        if m == 0 {
            break;
        }
    }
    if n < 0 {
        out.push(b'-');
    }
    out.extend_from_slice(&buf[k..]);
}

/// `pdf_print_real(m, d)` (pdfTeX §690): `m/10^d`, without trailing zeros.
pub fn real(out: &mut Vec<u8>, m: i32, d: i32) {
    let mut m = i64::from(m);
    let mut d = usize::try_from(d).unwrap_or(0);
    if m < 0 {
        out.push(b'-');
        m = -m;
    }
    int(out, m / TEN_POW[d]);
    m %= TEN_POW[d];
    if m > 0 {
        out.push(b'.');
        d -= 1;
        while m < TEN_POW[d] {
            out.push(b'0');
            d -= 1;
        }
        while m % 10 == 0 {
            m /= 10;
        }
        int(out, m);
    }
}

/// `pdf_print_bp(s)` (pdfTeX §690) with `fixed_decimal_digits` `dd`;
/// returns `scaled_out`.
pub fn bp(out: &mut Vec<u8>, s: Scaled, dd: i32) -> Scaled {
    let (q, so) = divide_scaled(s, ONE_HUNDRED_BP, dd + 2);
    real(out, q, dd);
    so
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `divide_scaled` with pdfTeX's digit loop, as written in §689.
    fn divide_scaled_loop(s: Scaled, m: Scaled, dd: i32) -> (i32, Scaled) {
        let (mut s, mut m) = (i64::from(s), i64::from(m));
        let mut sign = 1;
        if s < 0 {
            sign = -sign;
            s = -s;
        }
        if m < 0 {
            sign = -sign;
            m = -m;
        }
        let mut q = s / m;
        let mut r = s % m;
        for _ in 0..dd {
            q = 10 * q + (10 * r) / m;
            r = (10 * r) % m;
        }
        if 2 * r >= m {
            q += 1;
            r -= m;
        }
        let dd = usize::try_from(dd).unwrap_or(0);
        let out = sign * (s - r / TEN_POW[dd]);
        (narrow(sign * q), narrow(out))
    }

    #[test]
    fn int_prints_as_display() {
        for n in [0, 7, -7, 10, -10, 65536, -1234567, i64::MAX, i64::MIN] {
            let mut out = Vec::new();
            int(&mut out, n);
            assert_eq!(out, n.to_string().into_bytes());
        }
    }

    #[test]
    fn divide_scaled_is_the_digit_loop() {
        let mut ss: Vec<Scaled> = vec![
            0,
            1,
            2,
            5,
            9,
            10,
            32767,
            32768,
            65535,
            65536,
            65781,
            65782,
            6_578_175,
            6_578_176,
            1 << 29,
            (1 << 30) - 1,
            1 << 30,
            i32::MAX - 1,
            i32::MAX,
        ];
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        for _ in 0..4000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            ss.push(i32::try_from(x % (1 << 31)).unwrap_or(0) >> (x % 31));
        }
        let negs: Vec<Scaled> = ss.iter().map(|&s| -s).collect();
        ss.extend(negs);
        ss.push(i32::MIN + 1);
        let ms: Vec<Scaled> = vec![
            1,
            2,
            3,
            7,
            10,
            655,
            65536,
            65782,
            327_680,
            655_360,
            6_578_176,
            9_999_999,
            0x7FFF_FFFF / 10 - 1,
            -1,
            -65536,
            -6_578_176,
        ];
        for &s in &ss {
            for &m in &ms {
                for dd in -1..=9 {
                    assert_eq!(
                        divide_scaled(s, m, dd),
                        divide_scaled_loop(s, m, dd),
                        "s {s} m {m} dd {dd}"
                    );
                }
            }
        }
    }

    fn r(m: i32, d: i32) -> String {
        let mut v = Vec::new();
        real(&mut v, m, d);
        String::from_utf8(v).unwrap()
    }

    #[test]
    fn reals() {
        assert_eq!(r(99626, 4), "9.9626");
        assert_eq!(r(72000, 3), "72");
        assert_eq!(r(5, 3), "0.005");
        assert_eq!(r(-8856, 3), "-8.856");
        assert_eq!(r(150_100, 3), "150.1");
    }

    #[test]
    fn font_size() {
        // 10pt in hundredths of a bp, six digits
        assert_eq!(divide_scaled(655_360, ONE_HUNDRED_BP, 6).0, 99626);
    }
}
