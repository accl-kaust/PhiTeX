//! pdfTeX's numbers in PDF output (pdftex.web §689–§690, part 32b):
//! `divide_scaled`, `round_xn_over_d`, `pdf_print_int`, `pdf_print_real`,
//! `pdf_print_bp`, and the byte writer they print to. Pure functions.

/// 1bp, 100bp and 100in in scaled points (pdfTeX's "set initial values").
pub const ONE_BP: i32 = 65782;
pub const ONE_HUNDRED_BP: i32 = 6_578_176;
pub const ONE_HUNDRED_INCH: i32 = 473_628_672;

/// pdfTeX's `ten_pow`.
pub const TEN_POW: [i32; 10] = [1, 10, 100, 1000, 10_000, 100_000, 1_000_000, 10_000_000, 100_000_000, 1_000_000_000];

/// pdfTeX §689: `divide_scaled`: `(s/m) * 10^dd` rounded, and `scaled_out`
/// (the part of `s` that accounts for). `None`: pdfTeX's "divided by zero"
/// or "number too big" (fatal).
pub fn divide_scaled(s: i32, m: i32, dd: i32) -> Option<(i32, i32)> {
    let mut sign = 1;
    let (mut s, mut m) = (s, m);
    if s < 0 {
        sign = -sign;
        s = -s;
    }
    if m < 0 {
        sign = -sign;
        m = -m;
    }
    if m == 0 || m >= i32::MAX / 10 {
        return None;
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
    Some((sign * q, sign * (s - r / TEN_POW[dd.clamp(0, 9) as usize])))
}

/// pdfTeX §689: `round_xn_over_d` (as web2c computes it: in C integers).
pub fn round_xn_over_d(x: i32, n: i32, d: i32) -> i32 {
    let positive = x >= 0;
    let x = x.wrapping_abs();
    let t = (x % 0o100_000).wrapping_mul(n);
    let mut u = (x / 0o100_000).wrapping_mul(n).wrapping_add(t / 0o100_000);
    let v = (u % d).wrapping_mul(0o100_000).wrapping_add(t % 0o100_000);
    if u / d < 0o100_000 {
        u = (u / d).wrapping_mul(0o100_000).wrapping_add(v / d);
    }
    let v = v % d;
    if 2 * v >= d {
        u += 1;
    }
    if positive { u } else { -u }
}

/// Pascal's `round(x/10)` of an integer: halves away from zero.
pub fn round10(s: i32) -> i32 {
    if s >= 0 { (s + 5) / 10 } else { (s - 5) / 10 }
}

/// Bytes of PDF output, printed as pdfTeX prints them.
#[derive(Default, Clone, Debug)]
pub struct Out {
    pub b: Vec<u8>,
}

impl Out {
    pub fn new() -> Out {
        Out { b: Vec::new() }
    }

    pub fn out(&mut self, c: u8) {
        self.b.push(c);
    }

    pub fn print(&mut self, s: &[u8]) {
        self.b.extend_from_slice(s);
    }

    pub fn print_ln(&mut self, s: &[u8]) {
        self.b.extend_from_slice(s);
        self.b.push(b'\n');
    }

    pub fn nl(&mut self) {
        self.b.push(b'\n');
    }

    /// `pdf_print_int`.
    pub fn int(&mut self, n: i64) {
        if n < 0 {
            self.b.push(b'-');
        }
        let mut d = [0u8; 24];
        let mut k = 0;
        let mut m = n.unsigned_abs();
        loop {
            d[k] = (m % 10) as u8;
            m /= 10;
            k += 1;
            if m == 0 {
                break;
            }
        }
        while k > 0 {
            k -= 1;
            self.b.push(b'0' + d[k]);
        }
    }

    pub fn int_ln(&mut self, n: i64) {
        self.int(n);
        self.nl();
    }

    /// `pdf_print_fw_int`: the last `w` decimal digits of `n`.
    pub fn fw_int(&mut self, n: i64, w: usize) {
        let mut n = n;
        let mut d = vec![0u8; w];
        for x in &mut d {
            *x = n.rem_euclid(10) as u8;
            n /= 10;
        }
        for &c in d.iter().rev() {
            self.b.push(b'0' + c);
        }
    }

    /// `pdf_out_bytes`: the last `w` bytes of `n`, big-endian.
    pub fn bytes(&mut self, n: i64, w: usize) {
        let mut n = n;
        let mut d = vec![0u8; w];
        for x in &mut d {
            *x = n.rem_euclid(256) as u8;
            n /= 256;
        }
        for &c in d.iter().rev() {
            self.b.push(c);
        }
    }

    /// `pdf_print_real`: `m / 10^d`, trailing zeros dropped.
    pub fn real(&mut self, m: i32, d: i32) {
        let mut m = m;
        let mut d = d.clamp(0, 9);
        if m < 0 {
            self.b.push(b'-');
            m = -m;
        }
        let p = |d: i32| TEN_POW[d.clamp(0, 9) as usize];
        self.int(i64::from(m / p(d)));
        m %= p(d);
        if m > 0 {
            self.b.push(b'.');
            d -= 1;
            while m < p(d) {
                self.b.push(b'0');
                d -= 1;
            }
            while m % 10 == 0 {
                m /= 10;
            }
            self.int(i64::from(m));
        }
    }

    /// `pdf_print_bp`: `s` in big points with `dd` decimal digits; returns
    /// `scaled_out`.
    pub fn bp(&mut self, s: i32, dd: i32) -> i32 {
        let (q, out) = divide_scaled(s, ONE_HUNDRED_BP, dd + 2).unwrap_or((0, 0));
        self.real(q, dd);
        out
    }

    /// `pdf_print_mag_bp`.
    pub fn mag_bp(&mut self, s: i32, dd: i32, mag: i32) -> i32 {
        let s = if mag == 1000 { s } else { round_xn_over_d(s, mag, 1000) };
        self.bp(s, dd)
    }

    /// `pdf_print_octal` (at least three digits).
    pub fn octal(&mut self, n: u8) {
        if n < 8 {
            self.b.push(b'0');
        }
        if n < 64 {
            self.b.push(b'0');
        }
        self.b.extend(format!("{n:o}").bytes());
    }

    /// `pdf_indirect`.
    pub fn indirect(&mut self, s: &[u8], o: u32) {
        self.b.push(b'/');
        self.b.extend_from_slice(s);
        self.b.push(b' ');
        self.int(i64::from(o));
        self.b.extend_from_slice(b" 0 R");
    }

    pub fn indirect_ln(&mut self, s: &[u8], o: u32) {
        self.indirect(s, o);
        self.nl();
    }

    /// `pdf_int_entry_ln`.
    pub fn int_entry_ln(&mut self, s: &[u8], v: i64) {
        self.b.push(b'/');
        self.b.extend_from_slice(s);
        self.b.push(b' ');
        self.int(v);
        self.nl();
    }

    /// `pdf_print_str`: `s` as a PDF string (as it is if it is one).
    pub fn str(&mut self, s: &[u8]) {
        if s.is_empty() {
            self.print(b"()");
            return;
        }
        if s[0] == b'(' && s[s.len() - 1] == b')' {
            self.print(s);
            return;
        }
        let hex = s[0] == b'<'
            && s[s.len() - 1] == b'>'
            && s.len().is_multiple_of(2)
            && s[1..s.len() - 1].chunks(2).all(|p| p.iter().all(u8::is_ascii_hexdigit));
        if hex {
            self.print(s);
        } else {
            self.out(b'(');
            self.print(s);
            self.out(b')');
        }
    }

    /// `pdf_str_entry_ln`.
    pub fn str_entry_ln(&mut self, s: &[u8], v: &[u8]) {
        self.b.push(b'/');
        self.b.extend_from_slice(s);
        self.b.push(b' ');
        self.str(v);
        self.nl();
    }

    /// `remove_last_space`.
    pub fn remove_last_space(&mut self) {
        if self.b.last() == Some(&b' ') {
            self.b.pop();
        }
    }

    pub fn len(&self) -> usize {
        self.b.len()
    }

    pub fn is_empty(&self) -> bool {
        self.b.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_pdftex() {
        assert_eq!(divide_scaled(65536, ONE_HUNDRED_BP, 5), Some((996, 65519)));
        assert_eq!(divide_scaled(-65536, ONE_HUNDRED_BP, 5).map(|r| r.0), Some(-996));
        assert_eq!(divide_scaled(1, 0, 3), None);
        let mut o = Out::new();
        o.real(996, 3);
        o.out(b' ');
        o.real(-1500, 3);
        o.out(b' ');
        o.real(2000, 3);
        o.out(b' ');
        o.real(5, 3);
        assert_eq!(o.b, b"0.996 -1.5 2 0.005");
    }
}
