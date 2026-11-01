//! `flattenpath` (gxpflat.c): curves as lines, gs's subdivision count
//! for the flatness.

use crate::path::{P, Path, Pe};

/// The path with each curve replaced by `2^k` lines, `k` as gs's
/// `gx_curve_log2_samples` chooses for `flat` (in device pixels).
pub fn flatten(p: &Path, flat: f64) -> Path {
    let mut q = Path::new();
    let mut cur = P::default();
    for e in p.elements() {
        match e {
            Pe::Move(a) => {
                q.add_point(a);
                cur = a;
            }
            Pe::Line(a) => {
                let _ = q.add_line(a);
                cur = a;
            }
            Pe::Curve(a, b, c) => {
                let k = log2_samples(cur, a, b, c, flat);
                let n = 1i64 << k;
                for i in 1..=n {
                    let t = i as f64 / n as f64;
                    let mt = 1.0 - t;
                    let f = |p0: i32, p1: i32, p2: i32, p3: i32| {
                        let v = mt * mt * mt * f64::from(p0)
                            + 3.0 * mt * mt * t * f64::from(p1)
                            + 3.0 * mt * t * t * f64::from(p2)
                            + t * t * t * f64::from(p3);
                        libm::round(v) as i32
                    };
                    let pt = if i == n {
                        c
                    } else {
                        P {
                            x: f(cur.x, a.x, b.x, c.x),
                            y: f(cur.y, a.y, b.y, c.y),
                        }
                    };
                    let _ = q.add_line(pt);
                }
                cur = c;
            }
            Pe::Close(_) => q.close(),
        }
    }
    q
}

/// `gx_curve_log2_samples`.
fn log2_samples(p0: P, p1: P, p2: P, p3: P, flat: f64) -> u32 {
    let x03 = i64::from(p3.x - p0.x);
    let y03 = i64::from(p3.y - p0.y);
    let dx = (i64::from(p1.x) * 3 - i64::from(p0.x) * 2 - i64::from(p3.x))
        .abs()
        .max((i64::from(p2.x) * 3 - i64::from(p3.x) * 2 - i64::from(p0.x)).abs());
    let dy = (i64::from(p1.y) * 3 - i64::from(p0.y) * 2 - i64::from(p3.y))
        .abs()
        .max((i64::from(p2.y) * 3 - i64::from(p3.y) * 2 - i64::from(p0.y)).abs());
    let _ = (x03, y03);
    let d = dx.max(dy);
    let fixed_flat = (flat * 256.0) as i64;
    let mut k = 0;
    let mut d = d;
    if fixed_flat > 0 {
        while d > fixed_flat && k < 10 {
            d >>= 2;
            k += 1;
        }
    }
    k
}
