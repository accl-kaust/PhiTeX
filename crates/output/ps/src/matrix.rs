//! Ghostscript's matrices (gsmatrix.c): single-precision coefficients,
//! arithmetic in double where gs does it, the float casts gs makes.

/// `gs_matrix`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct M {
    pub xx: f32,
    pub xy: f32,
    pub yx: f32,
    pub yy: f32,
    pub tx: f32,
    pub ty: f32,
}

pub const IDENT: M = M {
    xx: 1.0,
    xy: 0.0,
    yx: 0.0,
    yy: 1.0,
    tx: 0.0,
    ty: 0.0,
};

fn z(f: f32) -> bool {
    f == 0.0
}

/// `gs_sincos_t`.
pub struct SinCos {
    pub sin: f64,
    pub cos: f64,
    pub orthogonal: bool,
}

/// `gs_sincos_degrees`.
pub fn sincos_degrees(ang: f64) -> SinCos {
    let quot = ang / 90.0;
    if libm::floor(quot) == quot {
        const ISC: [f64; 5] = [0.0, 1.0, 0.0, -1.0, 0.0];
        let quads = (libm::fmod(quot, 4.0) as i32) & 3;
        SinCos {
            sin: ISC[quads as usize],
            cos: ISC[quads as usize + 1],
            orthogonal: true,
        }
    } else {
        let a = ang * (core::f64::consts::PI / 180.0);
        SinCos {
            sin: libm::sin(a),
            cos: libm::cos(a),
            orthogonal: false,
        }
    }
}

impl M {
    pub fn is_xxyy(&self) -> bool {
        z(self.xy) && z(self.yx)
    }
    pub fn is_xyyx(&self) -> bool {
        z(self.xx) && z(self.yy)
    }
    pub fn translation(dx: f64, dy: f64) -> M {
        M {
            tx: dx as f32,
            ty: dy as f32,
            ..IDENT
        }
    }
    pub fn scaling(sx: f64, sy: f64) -> M {
        M {
            xx: sx as f32,
            yy: sy as f32,
            ..IDENT
        }
    }
    pub fn rotation(ang: f64) -> M {
        let sc = sincos_degrees(ang);
        M {
            xx: sc.cos as f32,
            xy: sc.sin as f32,
            yx: -sc.sin as f32,
            yy: sc.cos as f32,
            tx: 0.0,
            ty: 0.0,
        }
    }
    /// `gs_matrix_multiply(self, m2)`: self × m2.
    pub fn mul(&self, m2: &M) -> M {
        let (xx1, yy1) = (f64::from(self.xx), f64::from(self.yy));
        let (tx1, ty1) = (f64::from(self.tx), f64::from(self.ty));
        let (xx2, yy2) = (f64::from(m2.xx), f64::from(m2.yy));
        let (xy2, yx2) = (f64::from(m2.xy), f64::from(m2.yx));
        let mut r = IDENT;
        if self.is_xxyy() {
            let mut tx = tx1 * xx2 + f64::from(m2.tx);
            let mut ty = ty1 * yy2 + f64::from(m2.ty);
            // (stored as float, then added to)
            r.tx = tx as f32;
            r.ty = ty as f32;
            if z(m2.xy) {
                r.xy = 0.0;
            } else {
                r.xy = (xx1 * xy2) as f32;
                ty = f64::from(r.ty) + tx1 * xy2;
                r.ty = ty as f32;
            }
            r.xx = (xx1 * xx2) as f32;
            if z(m2.yx) {
                r.yx = 0.0;
            } else {
                r.yx = (yy1 * yx2) as f32;
                tx = f64::from(r.tx) + ty1 * yx2;
                r.tx = tx as f32;
            }
            r.yy = (yy1 * yy2) as f32;
        } else {
            let (xy1, yx1) = (f64::from(self.xy), f64::from(self.yx));
            r.xx = (xx1 * xx2 + xy1 * yx2) as f32;
            r.xy = (xx1 * xy2 + xy1 * yy2) as f32;
            r.yy = (yx1 * xy2 + yy1 * yy2) as f32;
            r.yx = (yx1 * xx2 + yy1 * yx2) as f32;
            r.tx = (tx1 * xx2 + ty1 * yx2 + f64::from(m2.tx)) as f32;
            r.ty = (tx1 * xy2 + ty1 * yy2 + f64::from(m2.ty)) as f32;
        }
        r
    }
    /// `gs_matrix_invert`.
    pub fn invert(&self) -> Option<M> {
        let mut r = IDENT;
        if self.is_xxyy() {
            if z(self.xx) || z(self.yy) {
                return None;
            }
            r.xx = (1.0 / f64::from(self.xx)) as f32;
            r.tx = (-f64::from(r.xx) * f64::from(self.tx)) as f32;
            r.xy = 0.0;
            r.yx = 0.0;
            r.yy = (1.0 / f64::from(self.yy)) as f32;
            r.ty = (-f64::from(r.yy) * f64::from(self.ty)) as f32;
        } else {
            let (mxx, myy, mxy, myx) = (self.xx, self.yy, self.xy, self.yx);
            let (mtx, mty) = (self.tx, self.ty);
            let det = f64::from(mxx * myy) - f64::from(mxy * myx);
            if det == 0.0 {
                return None;
            }
            r.xx = (f64::from(myy) / det) as f32;
            r.xy = (f64::from(-mxy) / det) as f32;
            r.yx = (f64::from(-myx) / det) as f32;
            r.yy = (f64::from(mxx) / det) as f32;
            r.tx = ((f64::from(mty * myx) - f64::from(mtx * myy)) / det) as f32;
            r.ty = ((f64::from(mtx * mxy) - f64::from(mty * mxx)) / det) as f32;
        }
        Some(r)
    }
    /// `gs_matrix_translate`.
    pub fn translate(&self, dx: f64, dy: f64) -> M {
        let (x, y) = self.dtransform(dx, dy);
        let mut r = *self;
        r.tx = (f64::from(r.tx) + x) as f32;
        r.ty = (f64::from(r.ty) + y) as f32;
        r
    }
    /// `gs_matrix_scale`.
    pub fn scale(&self, sx: f64, sy: f64) -> M {
        M {
            xx: (f64::from(self.xx) * sx) as f32,
            xy: (f64::from(self.xy) * sx) as f32,
            yx: (f64::from(self.yx) * sy) as f32,
            yy: (f64::from(self.yy) * sy) as f32,
            tx: self.tx,
            ty: self.ty,
        }
    }
    /// `gs_matrix_rotate`.
    pub fn rotate(&self, ang: f64) -> M {
        let sc = sincos_degrees(ang);
        let (mxx, mxy) = (f64::from(self.xx), f64::from(self.xy));
        M {
            xx: (sc.cos * mxx + sc.sin * f64::from(self.yx)) as f32,
            xy: (sc.cos * mxy + sc.sin * f64::from(self.yy)) as f32,
            yx: (sc.cos * f64::from(self.yx) - sc.sin * mxx) as f32,
            yy: (sc.cos * f64::from(self.yy) - sc.sin * mxy) as f32,
            tx: self.tx,
            ty: self.ty,
        }
    }
    /// `gs_point_transform`.
    pub fn transform(&self, x: f64, y: f64) -> (f64, f64) {
        let mut px = f64::from((x * f64::from(self.xx)) as f32) + f64::from(self.tx);
        let mut py = f64::from((y * f64::from(self.yy)) as f32) + f64::from(self.ty);
        if !z(self.yx) {
            px += f64::from((y * f64::from(self.yx)) as f32);
        }
        if !z(self.xy) {
            py += f64::from((x * f64::from(self.xy)) as f32);
        }
        (px, py)
    }
    /// `gs_point_transform_inverse`.
    pub fn itransform(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        if self.is_xxyy() {
            if z(self.xx) || z(self.yy) {
                return None;
            }
            Some((
                (x - f64::from(self.tx)) / f64::from(self.xx),
                (y - f64::from(self.ty)) / f64::from(self.yy),
            ))
        } else if self.is_xyyx() {
            if z(self.xy) || z(self.yx) {
                return None;
            }
            Some((
                (y - f64::from(self.ty)) / f64::from(self.xy),
                (x - f64::from(self.tx)) / f64::from(self.yx),
            ))
        } else {
            Some(self.invert()?.transform(x, y))
        }
    }
    /// `gs_distance_transform`.
    pub fn dtransform(&self, dx: f64, dy: f64) -> (f64, f64) {
        let mut x = dx * f64::from(self.xx);
        let mut y = dy * f64::from(self.yy);
        if !z(self.yx) {
            x += dy * f64::from(self.yx);
        }
        if !z(self.xy) {
            y += dx * f64::from(self.xy);
        }
        (x, y)
    }
    /// `gs_distance_transform_inverse`.
    pub fn idtransform(&self, dx: f64, dy: f64) -> Option<(f64, f64)> {
        let (xx, xy, yx, yy) = (
            f64::from(self.xx),
            f64::from(self.xy),
            f64::from(self.yx),
            f64::from(self.yy),
        );
        if self.is_xxyy() {
            if z(self.xx) || z(self.yy) {
                return None;
            }
            Some((dx / xx, dy / yy))
        } else if self.is_xyyx() {
            if z(self.xy) || z(self.yx) {
                return None;
            }
            Some((dy / xy, dx / yx))
        } else {
            let det = xx * yy - xy * yx;
            if det == 0.0 {
                return None;
            }
            Some(((dx * yy - dy * yx) / det, (dy * xx - dx * xy) / det))
        }
    }
    pub fn to_vec(&self) -> [f32; 6] {
        [self.xx, self.xy, self.yx, self.yy, self.tx, self.ty]
    }
    pub fn from_slice(v: &[f64]) -> M {
        M {
            xx: v[0] as f32,
            xy: v[1] as f32,
            yx: v[2] as f32,
            yy: v[3] as f32,
            tx: v[4] as f32,
            ty: v[5] as f32,
        }
    }
}
