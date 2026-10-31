//! C's `printf` number formats as glibc prints them, for what Ghostscript
//! writes with `pprintg` (`%g`, and `%1.1f` / `%1.8f` where `%g` would
//! have an exponent, base/spprint.c) and `%d`.

use alloc::format;
use alloc::string::String;

/// `%g` (precision 6), exactly rounded as glibc rounds.
#[must_use]
pub fn g(v: f64) -> String {
    g_prec(v, 6)
}

/// `%.<p>g`.
#[must_use]
pub fn g_prec(v: f64, p: usize) -> String {
    if v.is_nan() {
        return if v.is_sign_negative() {
            "-nan".into()
        } else {
            "nan".into()
        };
    }
    if v.is_infinite() {
        return if v < 0.0 { "-inf".into() } else { "inf".into() };
    }
    let p = p.max(1);
    if v == 0.0 {
        return if v.is_sign_negative() {
            "-0".into()
        } else {
            "0".into()
        };
    }
    // (the exponent after rounding to p significant digits)
    let e = format!("{:.*e}", p - 1, v);
    let (mant, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let x: i32 = exp.parse().unwrap_or(0);
    if x < -4 || x >= p as i32 {
        let mut m = String::from(mant);
        strip_zeros(&mut m);
        let sign = if x < 0 { '-' } else { '+' };
        let ax = x.unsigned_abs();
        return format!("{m}e{sign}{ax:02}");
    }
    let decimals = (p as i32 - 1 - x).max(0) as usize;
    let mut s = format!("{v:.decimals$}");
    strip_zeros(&mut s);
    s
}

fn strip_zeros(s: &mut String) {
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
}

/// `%.<p>f`.
#[must_use]
pub fn f(v: f64, p: usize) -> String {
    format!("{v:.p$}")
}

/// Ghostscript's `pprintg1`: `%g`, or `%1.1f` (|v| > 1) / `%1.8f` where
/// `%g` would use an exponent.
#[must_use]
pub fn pg(v: f64) -> String {
    let s = g(v);
    if s.contains('e') {
        if v.abs() > 1.0 { f(v, 1) } else { f(v, 8) }
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printf_g() {
        assert_eq!(g(0.1), "0.1");
        assert_eq!(g(1000.0), "1000");
        assert_eq!(g(123_456.0), "123456");
        assert_eq!(g(1_234_567.0), "1.23457e+06");
        assert_eq!(g(0.0001), "0.0001");
        assert_eq!(g(0.00001), "1e-05");
        assert_eq!(g(2.5), "2.5");
        assert_eq!(g(-0.333_333_33), "-0.333333");
        assert_eq!(g(f64::from(0.1_f32)), "0.1");
        assert_eq!(pg(0.00001), "0.00001000");
        assert_eq!(pg(1e7), "10000000.0");
    }
}
