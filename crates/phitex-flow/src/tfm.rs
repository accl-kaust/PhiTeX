//! A font from its TFM file: a pure function of the file's bytes
//! (tex.web part 30). Widths, heights, depths and parameters; ligatures
//! and kerns are a later feature.

use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CharDims {
    pub w: i32,
    pub h: i32,
    pub d: i32,
}

/// A loaded font.
pub struct Font {
    pub name: String,
    pub checksum: u32,
    pub design_size: i32,
    pub size: i32,
    pub chars: Vec<Option<CharDims>>,
    /// `params[k]` is TeX's `\fontdimen k` (1 slant, 2 space, 3 stretch,
    /// 4 shrink, 5 x-height, 6 quad, 7 extra space).
    pub params: Vec<i32>,
}

impl Font {
    pub fn char(&self, c: u32) -> Option<CharDims> {
        self.chars.get(c as usize).copied().flatten()
    }

    pub fn param(&self, n: usize) -> i32 {
        self.params.get(n).copied().unwrap_or(0)
    }

    /// Parses a TFM file at its design size.
    pub fn parse(name: &str, b: &[u8]) -> Result<Font, String> {
        let hw = |i: usize| -> Result<usize, String> {
            let o = 2 * i;
            if o + 1 >= b.len() {
                return Err("TFM file too short".into());
            }
            Ok((usize::from(b[o]) << 8) | usize::from(b[o + 1]))
        };
        let (lf, lh, bc, ec) = (hw(0)?, hw(1)?, hw(2)?, hw(3)?);
        let (nw, nh, nd, ni, nl, nk, ne, np) = (
            hw(4)?,
            hw(5)?,
            hw(6)?,
            hw(7)?,
            hw(8)?,
            hw(9)?,
            hw(10)?,
            hw(11)?,
        );
        if lf * 4 > b.len() || lh < 2 || ec > 255 || bc > ec + 1 {
            return Err("bad TFM header".into());
        }
        let nc = ec + 1 - bc;
        if 6 + lh + nc + nw + nh + nd + ni + nl + nk + ne + np != lf {
            return Err("TFM lengths do not add up".into());
        }
        let word = |i: usize| -> [u8; 4] {
            let o = 4 * i;
            [b[o], b[o + 1], b[o + 2], b[o + 3]]
        };
        let checksum = u32::from_be_bytes(word(6));
        let design_size = i32::from_be_bytes(word(7)) >> 4;
        let ci = 6 + lh;
        let wt = ci + nc;
        let ht = wt + nw;
        let dt = ht + nh;
        let it = dt + nd;
        let lk = it + ni;
        let kt = lk + nl;
        let et = kt + nk;
        let pt = et + ne;
        // (tex.web 572: fix_words scaled by the size z)
        let mut z = design_size;
        let mut alpha: i32 = 16;
        while z >= 0o40_000_000 {
            z /= 2;
            alpha += alpha;
        }
        let beta = 256 / alpha;
        let alpha = alpha * z;
        let store = |w: [u8; 4]| -> i32 {
            let (a, bb, c, d) = (
                i32::from(w[0]),
                i32::from(w[1]),
                i32::from(w[2]),
                i32::from(w[3]),
            );
            let sw = (((d * z) / 256 + c * z) / 256 + bb * z) / beta;
            match a {
                0 => sw,
                255 => sw - alpha,
                _ => 0,
            }
        };
        let mut chars = vec![None; 256];
        for (c, slot) in chars.iter_mut().enumerate().take(ec + 1).skip(bc) {
            if c >= bc + nc {
                break;
            }
            let w = word(ci + c - bc);
            let wi = usize::from(w[0]);
            if wi == 0 {
                continue;
            }
            let hi = usize::from(w[1] >> 4);
            let di = usize::from(w[1] & 15);
            *slot = Some(CharDims {
                w: store(word(wt + wi)),
                h: store(word(ht + hi)),
                d: store(word(dt + di)),
            });
        }
        let mut params = vec![0; 8];
        for (k, p) in params.iter_mut().enumerate().take(np.min(7) + 1).skip(1) {
            let w = word(pt + k - 1);
            *p = if k == 1 {
                i32::from_be_bytes(w) >> 4
            } else {
                store(w)
            };
        }
        Ok(Font {
            name: name.to_string(),
            checksum,
            design_size,
            size: design_size,
            chars,
            params,
        })
    }
}

impl PartialEq for Font {
    fn eq(&self, o: &Self) -> bool {
        std::ptr::eq(self, o)
            || (self.name == o.name
                && self.checksum == o.checksum
                && self.size == o.size
                && self.chars == o.chars
                && self.params == o.params)
    }
}

impl Eq for Font {}

impl fmt::Debug for Font {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Font({})", self.name)
    }
}
