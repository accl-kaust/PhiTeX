//! B5: TeX's arithmetic (tex.web 99–108; e-TeX expression rounding).
//!
//! Every function is pure. Where tex.web sets the global `arith_error`, the
//! `_checked` forms (and the `Option`-returning ones) return `None`; the plain
//! forms keep tex.web's result value in that case.

/// `half(x)` (tex.web 100): rounds halves up.
pub fn half(x: i32) -> i32 {
    if x & 1 == 1 { (x + 1).div_euclid(2) } else { x / 2 }
}

/// The largest legal dimension, `@'7777777777` (tex.web 421).
pub const INF_DIMEN: i32 = 0o7777777777;
/// `inf_bad` (tex.web 108).
pub const INF_BAD: i32 = 10000;

/// `mult_and_add(n, x, y, max_answer)` (tex.web 105): `n*x+y`, `None` on overflow.
pub fn mult_and_add(n: i32, x: i32, y: i32, max_answer: i32) -> Option<i32> {
    let (mut n, mut x) = (n as i64, x as i64);
    let (y, max) = (y as i64, max_answer as i64);
    if n < 0 {
        x = -x;
        n = -n;
    }
    if n == 0 {
        return Some(y as i32);
    }
    // Pascal `div` truncates toward zero, as Rust's `/` does.
    if x <= (max - y) / n && -x <= (max + y) / n { Some((n * x + y) as i32) } else { None }
}

/// `nx_plus_y` (tex.web 105): dimensions, bounded by `@'7777777777`.
pub fn nx_plus_y(n: i32, x: i32, y: i32) -> Option<i32> {
    mult_and_add(n, x, y, INF_DIMEN)
}

/// `mult_integers` (tex.web 105): integers, bounded by `@'17777777777`.
pub fn mult_integers(n: i32, x: i32) -> Option<i32> {
    mult_and_add(n, x, 0, i32::MAX)
}

/// `x_over_n` (tex.web 106): (quotient, remainder), truncating toward zero;
/// the remainder has the sign of `x` (negated when `n < 0`). `n == 0` gives
/// `(0, x)` (tex.web's arith_error case; see [`x_over_n_checked`]).
pub fn x_over_n(x: i32, n: i32) -> (i32, i32) {
    x_over_n_checked(x, n).unwrap_or((0, x))
}

pub fn x_over_n_checked(x: i32, n: i32) -> Option<(i32, i32)> {
    if n == 0 {
        return None;
    }
    let (mut x, mut n) = (x as i64, n as i64);
    let negative = n < 0;
    if negative {
        x = -x;
        n = -n;
    }
    let (q, mut r) = if x >= 0 { (x / n, x % n) } else { (-((-x) / n), -((-x) % n)) };
    if negative {
        r = -r;
    }
    Some((q as i32, r as i32))
}

/// `xn_over_d` (tex.web 107): `x*n/d` for `n, d` nonnegative, `n, d < 2^16`
/// (tex.web's precondition), truncated; (quotient, remainder).
pub fn xn_over_d(x: i32, n: i32, d: i32) -> (i32, i32) {
    match xn_over_d_checked(x, n, d) {
        Some(r) => r,
        None => {
            // tex.web leaves `u` as the partial product here; callers that
            // do not check the error get the same garbage TeX would.
            let positive = x >= 0;
            let ax = (x as i64).abs();
            let t = (ax % 0o100000) * n as i64;
            let u = (ax / 0o100000) * n as i64 + t / 0o100000;
            let v = if d != 0 { (u % d as i64) * 0o100000 + t % 0o100000 } else { 0 };
            let rem = if d != 0 { v % d as i64 } else { 0 };
            if positive { (u as i32, rem as i32) } else { (-(u as i32), -(rem as i32)) }
        }
    }
}

pub fn xn_over_d_checked(x: i32, n: i32, d: i32) -> Option<(i32, i32)> {
    if d == 0 {
        return None;
    }
    let positive = x >= 0;
    let p = (x as i64).abs() * n as i64;
    let (u, r) = (p / d as i64, p % d as i64);
    if u >= 0o10000000000 {
        return None;
    }
    if positive { Some((u as i32, r as i32)) } else { Some((-(u as i32), -(r as i32))) }
}

/// `badness(t, s)` (tex.web 108): about `100(t/s)^3`, at most `inf_bad`.
pub fn badness(t: i32, s: i32) -> i32 {
    if t == 0 {
        return 0;
    }
    if s <= 0 {
        return INF_BAD;
    }
    let r: i64 = if t <= 7_230_584 {
        (t as i64 * 297) / s as i64
    } else if s >= 1_663_497 {
        t as i64 / (s as i64 / 297)
    } else {
        t as i64
    };
    if r > 1290 { INF_BAD } else { ((r * r * r + 0o400000) / 0o1000000) as i32 }
}

// ---- e-TeX expressions (etex.ch "Declare subprocedures for scan_expr")

/// `add_or_sub(x, y, max_answer, negative)`: `x ± y`, `None` past `max_answer`.
pub fn add_or_sub(x: i32, y: i32, max_answer: i32, negative: bool) -> Option<i32> {
    let (x, max) = (x as i64, max_answer as i64);
    let y = if negative { -(y as i64) } else { y as i64 };
    let ok = if x >= 0 { y <= max - x } else { y >= -max - x };
    if ok { Some((x + y) as i32) } else { None }
}

/// `quotient(n, d)`: `n/d` rounded, halves away from zero; `None` if `d = 0`.
pub fn quotient(n: i32, d: i32) -> Option<i32> {
    if d == 0 {
        return None;
    }
    let (mut n, mut d) = (n as i64, d as i64);
    let mut negative = false;
    if d < 0 {
        d = -d;
        negative = true;
    }
    if n < 0 {
        n = -n;
        negative = !negative;
    }
    let mut a = n / d;
    let rem = n - a * d;
    if rem - d + rem >= 0 {
        a += 1;
    }
    if negative {
        a = -a;
    }
    Some(a as i32)
}

/// `fract(x, n, d, max_answer)`: `x*n/d` rounded (halves away from zero),
/// `None` if `d = 0` or the result exceeds `max_answer`.
pub fn fract(x: i32, n: i32, d: i32, max_answer: i32) -> Option<i32> {
    if d == 0 {
        return None;
    }
    let num = x as i128 * n as i128;
    let d = d as i128;
    let negative = (num < 0) != (d < 0);
    let (num, d) = (num.abs(), d.abs());
    let mut a = num / d;
    let rem = num % d;
    if 2 * rem >= d {
        a += 1;
    }
    if a > max_answer as i128 {
        return None;
    }
    Some(if negative { -(a as i32) } else { a as i32 })
}

/// e-TeX `\numexpr` bound: `infinity` = `@'17777777777`.
pub const INFINITY: i32 = i32::MAX;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn basics() {
        assert_eq!(badness(0, 5), 0);
        assert_eq!(badness(10, 0), INF_BAD);
        assert_eq!(badness(100, 100), 100);
        assert_eq!(x_over_n(-7, 2), (-3, -1));
        assert_eq!(x_over_n(7, -2), (-3, -1));
        assert_eq!(quotient(7, 2), Some(4));
        assert_eq!(quotient(-7, 2), Some(-4));
        assert_eq!(xn_over_d(65536, 3, 2), (98304, 0));
        assert_eq!(nx_plus_y(2, INF_DIMEN, 0), None);
        assert_eq!(fract(3, 1, 2, INFINITY), Some(2));
    }
}
