//! A smooth shading as the browser draws it: axial and radial shadings
//! (types 2 and 3: `TikZ`'s and `pgf`'s, `matplotlib`'s gradients) sampled
//! into an RGBA image over the area they paint, drawn as the page's other
//! images (the draw list's `"I"` and `"r"`, inside the clip in force).

// (pixels: small integers; colours: 0 to 255; the specification's
// short names: t, s, x, y, a, b, c)
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::many_single_char_names
)]

/// A PDF function of one input (the shadings' `t`), as types 0, 2 and 3
/// give it, or an array of them (one output each).
#[derive(Clone, Debug)]
pub(crate) enum Func {
    /// Type 2: `c0 + t^n (c1 - c0)`.
    Exp {
        domain: [f64; 2],
        c0: Vec<f64>,
        c1: Vec<f64>,
        n: f64,
    },
    /// Type 3: functions stitched at `bounds`.
    Stitch {
        domain: [f64; 2],
        fns: Vec<Func>,
        bounds: Vec<f64>,
        encode: Vec<f64>,
    },
    /// Type 0, one input: samples (outputs each, decoded) linearly
    /// interpolated.
    Sampled {
        domain: [f64; 2],
        encode: [f64; 2],
        samples: Vec<Vec<f64>>,
    },
    /// One function an output.
    Many(Vec<Func>),
}

fn interp(x: f64, a: f64, b: f64, c: f64, d: f64) -> f64 {
    if (b - a).abs() < 1e-12 {
        c
    } else {
        c + (x - a) * (d - c) / (b - a)
    }
}

impl Func {
    pub(crate) fn eval(&self, t: f64) -> Vec<f64> {
        match self {
            Func::Exp { domain, c0, c1, n } => {
                let t = t.clamp(domain[0], domain[1]);
                let k = if (*n - 1.0).abs() < 1e-12 {
                    t
                } else {
                    t.powf(*n)
                };
                c0.iter().zip(c1).map(|(a, b)| a + k * (b - a)).collect()
            }
            Func::Stitch {
                domain,
                fns,
                bounds,
                encode,
            } => {
                let t = t.clamp(domain[0], domain[1]);
                let i = bounds.iter().take_while(|&&b| t >= b).count();
                let i = i.min(fns.len().saturating_sub(1));
                let lo = if i == 0 { domain[0] } else { bounds[i - 1] };
                let hi = bounds.get(i).copied().unwrap_or(domain[1]);
                let e0 = encode.get(2 * i).copied().unwrap_or(0.0);
                let e1 = encode.get(2 * i + 1).copied().unwrap_or(1.0);
                fns.get(i)
                    .map_or_else(Vec::new, |f| f.eval(interp(t, lo, hi, e0, e1)))
            }
            Func::Sampled {
                domain,
                encode,
                samples,
            } => {
                if samples.is_empty() {
                    return Vec::new();
                }
                let t = t.clamp(domain[0], domain[1]);
                let last = (samples.len() - 1) as f64;
                let e = interp(t, domain[0], domain[1], encode[0], encode[1]).clamp(0.0, last);
                let (i, f) = (e.floor() as usize, e.fract());
                let a = &samples[i];
                let b = &samples[(i + 1).min(samples.len() - 1)];
                a.iter().zip(b).map(|(a, b)| a + f * (b - a)).collect()
            }
            Func::Many(fs) => fs
                .iter()
                .filter_map(|f| f.eval(t).first().copied())
                .collect(),
        }
    }
}

/// An axial (`radial` false: `coords` x0 y0 x1 y1) or radial (x0 y0 r0
/// x1 y1 r1) shading, its colours made RGB by `rgb`.
pub(crate) struct Shading {
    pub radial: bool,
    pub coords: Vec<f64>,
    pub domain: [f64; 2],
    pub extend: [bool; 2],
    pub func: Func,
}

impl Shading {
    /// Where on the shading's axis point (x, y) of its space is, from 0
    /// to 1 (None: outside what it paints).
    fn s(&self, x: f64, y: f64) -> Option<f64> {
        let c = &self.coords;
        let ok = |s: f64| {
            if s < 0.0 {
                self.extend[0].then_some(0.0)
            } else if s > 1.0 {
                self.extend[1].then_some(1.0)
            } else {
                Some(s)
            }
        };
        if !self.radial {
            let (dx, dy) = (c[2] - c[0], c[3] - c[1]);
            let l = dx * dx + dy * dy;
            if l < 1e-12 {
                return None;
            }
            return ok(((x - c[0]) * dx + (y - c[1]) * dy) / l);
        }
        // (the largest s whose circle, of radius r(s) ≥ 0, goes through the
        // point: |p - c(s)| = r(s))
        let (cx, cy, dr) = (c[3] - c[0], c[4] - c[1], c[5] - c[2]);
        let (px, py, r0) = (x - c[0], y - c[1], c[2]);
        let a = cx * cx + cy * cy - dr * dr;
        let b = px * cx + py * cy + r0 * dr;
        let cc = px * px + py * py - r0 * r0;
        let try_s = |s: f64| (r0 + s * dr >= 0.0).then(|| ok(s)).flatten();
        if a.abs() < 1e-12 {
            if b.abs() < 1e-12 {
                return None;
            }
            return try_s(cc / (2.0 * b));
        }
        let disc = b * b - a * cc;
        if disc < 0.0 {
            return None;
        }
        let q = disc.sqrt();
        let (s1, s2) = ((b + q) / a, (b - q) / a);
        let (hi, lo) = if s1 >= s2 { (s1, s2) } else { (s2, s1) };
        try_s(hi).or_else(|| try_s(lo))
    }

    /// The shading over the device rectangle (`x0`, `y0`) `w` × `h`
    /// (points, y down) as `pw` × `ph` RGBA pixels; `to_shading` takes a
    /// device point to the shading's space, `rgb` its colour components
    /// to RGB.
    pub(crate) fn raster(
        &self,
        rect: [f64; 4],
        (pw, ph): (usize, usize),
        to_shading: &dyn Fn(f64, f64) -> (f64, f64),
        rgb: &dyn Fn(&[f64]) -> [u8; 3],
    ) -> Vec<u8> {
        // (colours along the axis, sampled once)
        const N: usize = 1024;
        let [x0, y0, w, h] = rect;
        let mut out = vec![0u8; pw * ph * 4];
        let lut: Vec<[u8; 3]> = (0..=N)
            .map(|i| {
                let s = i as f64 / N as f64;
                let t = self.domain[0] + s * (self.domain[1] - self.domain[0]);
                rgb(&self.func.eval(t))
            })
            .collect();
        for j in 0..ph {
            for i in 0..pw {
                let dx = x0 + (i as f64 + 0.5) * w / pw as f64;
                let dy = y0 + (j as f64 + 0.5) * h / ph as f64;
                let (x, y) = to_shading(dx, dy);
                if let Some(s) = self.s(x, y) {
                    let c = lut[(s * N as f64).round() as usize];
                    let k = (j * pw + i) * 4;
                    out[k..k + 3].copy_from_slice(&c);
                    out[k + 3] = 255;
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn functions_and_axes() {
        let red_blue = Func::Exp {
            domain: [0.0, 1.0],
            c0: vec![1.0, 0.0, 0.0],
            c1: vec![0.0, 0.0, 1.0],
            n: 1.0,
        };
        assert_eq!(red_blue.eval(0.25), vec![0.75, 0.0, 0.25]);
        let st = Func::Stitch {
            domain: [0.0, 1.0],
            fns: vec![red_blue.clone(), red_blue.clone()],
            bounds: vec![0.5],
            encode: vec![0.0, 1.0, 1.0, 0.0],
        };
        assert_eq!(st.eval(0.25), vec![0.5, 0.0, 0.5]);
        assert_eq!(st.eval(1.0), vec![1.0, 0.0, 0.0]);
        let axial = Shading {
            radial: false,
            coords: vec![0.0, 0.0, 10.0, 0.0],
            domain: [0.0, 1.0],
            extend: [false, true],
            func: red_blue.clone(),
        };
        assert_eq!(axial.s(5.0, 3.0), Some(0.5));
        assert_eq!(axial.s(-1.0, 0.0), None);
        assert_eq!(axial.s(20.0, 0.0), Some(1.0));
        let radial = Shading {
            radial: true,
            coords: vec![0.0, 0.0, 0.0, 0.0, 0.0, 10.0],
            domain: [0.0, 1.0],
            extend: [false, false],
            func: red_blue,
        };
        assert!((radial.s(3.0, 4.0).unwrap() - 0.5).abs() < 1e-9);
        assert_eq!(radial.s(30.0, 0.0), None);
    }
}
