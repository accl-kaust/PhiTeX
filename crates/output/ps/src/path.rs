//! Ghostscript's paths (gxpath.c): segments in device space as `fixed`
//! (24.8), a moveto kept as a position until something is drawn from it,
//! a trailing moveto enumerated at the end; arcs as gs makes them
//! (gspath1.c).

use alloc::vec::Vec;

pub type Fixed = i32;
pub const FIXED_SCALE: f64 = 256.0;

/// `float2fixed`: truncation.
pub fn f2x(f: f64) -> Fixed {
    (f * FIXED_SCALE) as Fixed
}
/// `float2fixed_rounded`.
pub fn f2x_round(f: f64) -> Fixed {
    libm::floor(f * FIXED_SCALE + 0.5) as Fixed
}
/// `fixed2float`.
pub fn x2f(x: Fixed) -> f64 {
    f64::from(x) * (1.0 / FIXED_SCALE)
}
/// `f_fits_in_bits(f, 24)`.
pub fn fits(f: f64) -> bool {
    let lim = f64::from(1u32 << 23);
    f >= -lim && f < lim
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct P {
    pub x: Fixed,
    pub y: Fixed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Seg {
    Start(P),
    Line(P),
    Curve(P, P, P),
    Close(P),
}

impl Seg {
    pub fn pt(&self) -> P {
        match *self {
            Seg::Start(p) | Seg::Line(p) | Seg::Close(p) | Seg::Curve(_, _, p) => p,
        }
    }
}

/// An enumerated path element (`gs_pe_*`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pe {
    Move(P),
    Line(P),
    Curve(P, P, P),
    Close(P),
}

const PSF_POSITION_VALID: u8 = 1;
const PSF_SUBPATH_OPEN: u8 = 2;
const PSF_IS_DRAWING: u8 = 4;
const LAST_MOVETO: u8 = PSF_POSITION_VALID | PSF_SUBPATH_OPEN;
const LAST_DRAW: u8 = LAST_MOVETO | PSF_IS_DRAWING;
const LAST_CLOSEPATH: u8 = PSF_POSITION_VALID;

/// `gx_path`.
#[derive(Clone, Debug, Default)]
pub struct Path {
    pub segs: Vec<Seg>,
    pub position: P,
    pub flags: u8,
    /// The index of the current subpath's start segment.
    pub cur_sub: Option<usize>,
    pub subpaths: usize,
    pub curves: usize,
}

pub enum PathErr {
    NoCurrentPoint,
}

impl Path {
    pub fn new() -> Path {
        Path::default()
    }
    pub fn position_valid(&self) -> bool {
        self.flags & PSF_POSITION_VALID != 0
    }
    pub fn last_is_moveto(&self) -> bool {
        self.flags == LAST_MOVETO
    }
    pub fn is_void(&self) -> bool {
        self.subpaths == 0
    }
    pub fn is_null(&self) -> bool {
        self.is_void() && !self.position_valid()
    }
    fn new_subpath(&mut self) {
        self.cur_sub = Some(self.segs.len());
        self.segs.push(Seg::Start(self.position));
        self.subpaths += 1;
    }
    fn open(&mut self) -> Result<(), PathErr> {
        if self.flags & PSF_IS_DRAWING != 0 {
            return Ok(());
        }
        if self.position_valid() {
            self.new_subpath();
            Ok(())
        } else {
            Err(PathErr::NoCurrentPoint)
        }
    }
    pub fn add_point(&mut self, p: P) {
        self.position = p;
        self.flags = LAST_MOVETO;
    }
    pub fn add_line(&mut self, p: P) -> Result<(), PathErr> {
        self.open()?;
        self.segs.push(Seg::Line(p));
        self.position = p;
        self.flags = LAST_DRAW;
        Ok(())
    }
    pub fn add_curve(&mut self, p1: P, p2: P, p3: P) -> Result<(), PathErr> {
        self.open()?;
        self.segs.push(Seg::Curve(p1, p2, p3));
        self.position = p3;
        self.curves += 1;
        self.flags = LAST_DRAW;
        Ok(())
    }
    pub fn close(&mut self) {
        if self.flags & PSF_SUBPATH_OPEN == 0 {
            return;
        }
        if self.last_is_moveto() {
            self.new_subpath();
        }
        let start = self.cur_sub.map_or(self.position, |i| self.segs[i].pt());
        self.segs.push(Seg::Close(start));
        self.position = start;
        self.flags = LAST_CLOSEPATH;
    }
    /// The path's elements as `gx_path_enum_next` gives them.
    pub fn elements(&self) -> Vec<Pe> {
        let mut v: Vec<Pe> = self
            .segs
            .iter()
            .map(|s| match *s {
                Seg::Start(p) => Pe::Move(p),
                Seg::Line(p) => Pe::Line(p),
                Seg::Curve(a, b, c) => Pe::Curve(a, b, c),
                Seg::Close(p) => Pe::Close(p),
            })
            .collect();
        if self.last_is_moveto() {
            v.push(Pe::Move(self.position));
        }
        v
    }
    /// `gx_path_bbox`.
    pub fn bbox(&self) -> (P, P) {
        if self.segs.is_empty() {
            let p = if self.position_valid() {
                self.position
            } else {
                P::default()
            };
            return (p, p);
        }
        let p0 = self.segs[0].pt();
        let (mut px, mut py, mut qx, mut qy) = (p0.x, p0.y, p0.x, p0.y);
        let mut add = |p: P| {
            px = px.min(p.x);
            qx = qx.max(p.x);
            py = py.min(p.y);
            qy = qy.max(p.y);
        };
        for s in &self.segs[1..] {
            if let Seg::Curve(a, b, _) = s {
                add(*a);
                add(*b);
            }
            add(s.pt());
        }
        (P { x: px, y: py }, P { x: qx, y: qy })
    }
    /// `gx_path_is_rectangular`: the box and whether the subpath is
    /// closed (`prt_closed`), open (`prt_open`) or fake closed.
    pub fn is_rectangular(&self) -> Option<(P, P, Rect)> {
        if self.subpaths != 1 {
            return None;
        }
        subpath_is_rectangular(&self.segs, 0).map(|(a, b, t, _)| (a, b, t))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rect {
    Open,
    Closed,
    FakeClosed,
}

fn curve_is_point(x0: Fixed, y0: Fixed, s: &Seg) -> bool {
    match *s {
        Seg::Curve(a, b, c) => {
            a.x == x0 && a.y == y0 && b.x == x0 && b.y == y0 && c.x == x0 && c.y == y0
        }
        _ => false,
    }
}
fn curve_is_line(x0: Fixed, y0: Fixed, s: &Seg) -> bool {
    match *s {
        Seg::Curve(a, b, c) => (a.x == x0 && a.y == y0 && b.x == c.x && b.y == c.y),
        _ => false,
    }
}

/// `gx_subpath_is_rectangular` from segment `i` (a start): the box, its
/// type and the index of the next subpath.
pub fn subpath_is_rectangular(segs: &[Seg], i: usize) -> Option<(P, P, Rect, usize)> {
    let p0 = segs[i].pt();
    let (x0, y0) = (p0.x, p0.y);
    let mut k = i;
    // (each of the next three distinct points)
    let mut pts = [P::default(); 3];
    let mut prev = p0;
    for pt in &mut pts {
        loop {
            k += 1;
            let s = segs.get(k)?;
            let p = s.pt();
            match s {
                Seg::Curve(..) => {
                    if curve_is_point(prev.x, prev.y, s) {
                        continue;
                    }
                    if curve_is_line(prev.x, prev.y, s) {
                        *pt = p;
                        break;
                    }
                    return None;
                }
                Seg::Line(_) => {}
                _ => return None,
            }
            if p != prev {
                *pt = p;
                break;
            }
        }
        prev = *pt;
    }
    let [p1, p2, p3] = pts;
    let (x1, y1, x2, y2, x3, y3) = (p1.x, p1.y, p2.x, p2.y, p3.x, p3.y);
    let ty;
    loop {
        k += 1;
        let s = segs.get(k);
        match s {
            None | Some(Seg::Start(_)) => {
                ty = Rect::Open;
                break;
            }
            Some(s @ Seg::Curve(..)) => {
                if curve_is_point(x3, y3, s) {
                    continue;
                }
                if curve_is_line(x3, y3, s) {
                    let p = s.pt();
                    if p != p0 {
                        return None;
                    }
                    match segs.get(k + 1) {
                        None | Some(Seg::Start(_)) => {
                            ty = Rect::FakeClosed;
                            break;
                        }
                        _ => return None,
                    }
                }
                return None;
            }
            Some(Seg::Close(_)) => {
                ty = Rect::Closed;
                break;
            }
            Some(s) => {
                let p = s.pt();
                if p.x == x3 && p.y == y3 {
                    continue;
                }
                if p != p0 {
                    return None;
                }
                match segs.get(k + 1) {
                    None | Some(Seg::Start(_)) => {
                        ty = Rect::FakeClosed;
                        break;
                    }
                    _ => return None,
                }
            }
        }
    }
    if (x0 == x1 && y1 == y2 && x2 == x3 && y3 == y0)
        || (x0 == x3 && y3 == y2 && x2 == x1 && y1 == y0)
    {
        let (px, qx) = if x0 < x2 { (x0, x2) } else { (x2, x0) };
        let (py, qy) = if y0 < y2 { (y0, y2) } else { (y2, y0) };
        let mut n = k;
        while n < segs.len() && !matches!(segs[n], Seg::Start(_)) {
            n += 1;
        }
        return Some((P { x: px, y: py }, P { x: qx, y: qy }, ty, n));
    }
    None
}
