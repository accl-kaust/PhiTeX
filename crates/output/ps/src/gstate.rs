//! The graphics state (gsstate.c, gscoord.c, gspath.c, gxcpath.c) and
//! its operators: the CTM, the path, the clip, line parameters; colors
//! are `crate::color`'s.

use alloc::rc::Rc;
use alloc::vec;
use alloc::vec::Vec;

use crate::color::Color;
use crate::interp::{E, Frame, Ps, R, Sig};
use crate::matrix::{IDENT, M};
use crate::obj::{Obj, V};
use crate::path::{P, Path, Pe, f2x, f2x_round, fits, x2f};

/// A clipping path list element (`gx_cpath_path_list`).
#[derive(Clone, Debug)]
pub struct ClipList {
    pub path: Path,
    pub rule: i32,
    pub next: Option<Rc<ClipList>>,
}

/// `gx_clip_path`, as much of it as the PDF writer reads.
#[derive(Clone, Debug)]
pub struct Clip {
    pub id: u64,
    /// The rectangle list is one rectangle: it.
    pub rect: Option<(P, P)>,
    pub inner: (P, P),
    pub outer: (P, P),
    pub path_valid: bool,
    pub path: Path,
    pub rule: i32,
    pub list: Option<Rc<ClipList>>,
}

impl Clip {
    pub fn rect(id: u64, a: P, b: P) -> Clip {
        Clip {
            id,
            rect: Some((a, b)),
            inner: (a, b),
            outer: (a, b),
            path_valid: false,
            path: Path::new(),
            rule: 0,
            list: None,
        }
    }
}

/// Line parameters.
#[derive(Clone, Debug)]
pub struct Line {
    pub half_width: f32,
    pub cap: i32,
    pub join: i32,
    pub miter_limit: f32,
    pub dash: Vec<f32>,
    pub dash_offset: f32,
}

impl Default for Line {
    fn default() -> Line {
        Line {
            half_width: 0.5,
            cap: 0,
            join: 0,
            miter_limit: 10.0,
            dash: Vec::new(),
            dash_offset: 0.0,
        }
    }
}

/// `gs_gstate`.
#[derive(Clone)]
pub struct GState {
    pub ctm: M,
    pub path: Path,
    /// The current point in device space (doubles, `PRECISE_CURRENTPOINT`).
    pub cur: Option<(f64, f64)>,
    pub subpath_start: (f64, f64),
    pub clip: Clip,
    pub line: Line,
    pub flatness: f32,
    pub smoothness: f32,
    pub stroke_adjust: bool,
    pub overprint: bool,
    pub color: Color,
    pub font: Option<Rc<crate::font::Font>>,
    /// The font as a dictionary (`currentfont`).
    pub font_dict: Option<Obj>,
    pub transfer: Option<Obj>,
    pub halftone_id: u32,
    /// Whether this gstate is in a `BuildChar`'s cached character.
    pub in_cachedevice: bool,
    pub char_width: Option<(f64, f64)>,
}

impl Default for GState {
    fn default() -> GState {
        GState {
            ctm: IDENT,
            path: Path::new(),
            cur: None,
            subpath_start: (0.0, 0.0),
            clip: Clip::rect(0, P::default(), P::default()),
            line: Line::default(),
            flatness: 1.0,
            smoothness: 0.02,
            stroke_adjust: false,
            overprint: false,
            color: Color::default(),
            font: None,
            font_dict: None,
            transfer: None,
            halftone_id: 1,
            in_cachedevice: false,
            char_width: None,
        }
    }
}

/// Replaces the current gstate (`grestore`, `setgstate`, `restore`).
pub fn set_gstate(ps: &mut Ps<'_>, g: GState) {
    ps.gs = g;
}

impl Ps<'_> {
    pub fn next_id(&mut self) -> u64 {
        self.dev.ids += 1;
        self.dev.ids
    }

    /// The device's page rectangle in fixed.
    pub fn page_rect(&self) -> (P, P) {
        (
            P::default(),
            P {
                x: self.dev.width << 8,
                y: self.dev.height << 8,
            },
        )
    }

    /// `gs_initclip`.
    pub fn initclip(&mut self) {
        let (a, b) = self.page_rect();
        let id = self.next_id();
        self.gs.clip = Clip::rect(id, a, b);
    }

    /// `gs_initgraphics`.
    pub fn initgraphics(&mut self) {
        self.gs.ctm = self.dev.default_matrix();
        self.gs.path = Path::new();
        self.gs.cur = None;
        self.initclip();
        self.gs.line = Line::default();
        self.gs.color = Color::default();
        self.gs.stroke_adjust = false;
    }

    // ------ points and paths ------

    fn clamp_point(x: f64, y: f64) -> R<P> {
        if !fits(x) || !fits(y) {
            return Err(E::LimitCheck.into());
        }
        Ok(P {
            x: f2x_round(x),
            y: f2x_round(y),
        })
    }

    pub fn moveto_dev(&mut self, x: f64, y: f64) -> R {
        let p = Self::clamp_point(x, y)?;
        self.gs.path.add_point(p);
        self.gs.cur = Some((x, y));
        self.gs.subpath_start = (x, y);
        Ok(())
    }
    pub fn lineto_dev(&mut self, x: f64, y: f64) -> R {
        let p = Self::clamp_point(x, y)?;
        self.gs
            .path
            .add_line(p)
            .map_err(|_| Sig::Err(E::NoCurrentPoint))?;
        self.gs.cur = Some((x, y));
        Ok(())
    }
    pub fn curveto_dev(&mut self, c: [f64; 6]) -> R {
        let p1 = Self::clamp_point(c[0], c[1])?;
        let p2 = Self::clamp_point(c[2], c[3])?;
        let p3 = Self::clamp_point(c[4], c[5])?;
        self.gs
            .path
            .add_curve(p1, p2, p3)
            .map_err(|_| Sig::Err(E::NoCurrentPoint))?;
        self.gs.cur = Some((c[4], c[5]));
        Ok(())
    }
    pub fn closepath(&mut self) {
        self.gs.path.close();
        if self.gs.cur.is_some() {
            self.gs.cur = Some(self.gs.subpath_start);
        }
    }
    pub fn newpath(&mut self) {
        self.gs.path = Path::new();
        self.gs.cur = None;
    }
    pub fn currentpoint(&self) -> R<(f64, f64)> {
        let (x, y) = self.gs.cur.ok_or(Sig::Err(E::NoCurrentPoint))?;
        self.gs
            .ctm
            .itransform(x, y)
            .ok_or(Sig::Err(E::UndefinedResult))
    }

    /// `gs_gstate_arc_add` (gspath1.c).
    pub fn arc_add(
        &mut self,
        clockwise: bool,
        xc: f64,
        yc: f64,
        rad: f64,
        a1: f64,
        a2: f64,
        add_line: bool,
    ) -> R {
        let mut ar = rad;
        let (mut ang1, mut ang2) = (a1, a2);
        if ar < 0.0 {
            ang1 += 180.0;
            ang2 += 180.0;
            ar = -ar;
        }
        let mut arc = Arc {
            radius: ar,
            center: (xc, yc),
            action: if add_line {
                Action::Lineto
            } else {
                Action::Moveto
            },
            p0: (0.0, 0.0),
            p3: (0.0, 0.0),
            pt: (0.0, 0.0),
            sin: 0.0,
            cos: 0.0,
            angle: 0.0,
            fast: 0,
            quadrant_delta: 0,
        };
        let ang1r = libm::fmod(ang1, 360.0);
        let sc = crate::matrix::sincos_degrees(ang1r);
        arc.sin = sc.sin;
        arc.cos = sc.cos;
        arc.p3 = (xc + ar * sc.cos, yc + ar * sc.sin);
        let p3;
        'last: {
            if clockwise {
                if ang1 < ang2 {
                    ang2 -= libm::ceil((ang2 - ang1) / 360.0) * 360.0;
                }
                if ang2 < 0.0 {
                    let adj = libm::ceil(-ang2 / 360.0) * 360.0;
                    ang1 += adj;
                    ang2 += adj;
                }
                arc.angle = ang1;
                if ang1 == ang2 {
                    break 'last;
                }
                if !sc.orthogonal {
                    let anext = libm::floor(arc.angle / 90.0) * 90.0;
                    if anext < ang2 {
                        break 'last;
                    }
                    self.next_arc_curve(&mut arc, anext)?;
                    arc.action = Action::Nothing;
                }
                loop {
                    let anext = arc.angle - 90.0;
                    if anext < ang2 {
                        break;
                    }
                    self.next_arc_quadrant(&mut arc, anext)?;
                    arc.action = Action::Nothing;
                }
            } else {
                if ang2 < ang1 {
                    ang2 += libm::ceil((ang1 - ang2) / 360.0) * 360.0;
                }
                if ang1 < 0.0 {
                    let adj = libm::ceil(-ang1 / 360.0) * 360.0;
                    ang1 += adj;
                    ang2 += adj;
                }
                arc.angle = ang1;
                if ang1 == ang2 {
                    self.next_arc_curve(&mut arc, ang2)?;
                    let p = arc.p3;
                    return self.arc_done(p);
                }
                if !sc.orthogonal {
                    let anext = libm::ceil(arc.angle / 90.0) * 90.0;
                    if anext > ang2 {
                        break 'last;
                    }
                    self.next_arc_curve(&mut arc, anext)?;
                    arc.action = Action::Nothing;
                }
                loop {
                    let anext = arc.angle + 90.0;
                    if anext > ang2 {
                        break;
                    }
                    self.next_arc_quadrant(&mut arc, anext)?;
                    arc.action = Action::Nothing;
                }
            }
            if arc.angle == ang2 {
                let p = arc.p3;
                return self.arc_done(p);
            }
        }
        self.next_arc_curve(&mut arc, ang2)?;
        p3 = arc.p3;
        self.arc_done(p3)
    }

    fn arc_done(&mut self, p3: (f64, f64)) -> R {
        let (x, y) = self.gs.ctm.transform(p3.0, p3.1);
        self.gs.cur = Some((x, y));
        Ok(())
    }

    fn next_arc_curve(&mut self, arc: &mut Arc, anext: f64) -> R {
        let (x0, y0) = arc.p3;
        arc.p0 = arc.p3;
        let trad =
            arc.radius * libm::tan((anext - arc.angle) * (core::f64::consts::PI / 180.0 / 2.0));
        arc.pt = (x0 - trad * arc.sin, y0 + trad * arc.cos);
        let sc = crate::matrix::sincos_degrees(anext);
        arc.sin = sc.sin;
        arc.cos = sc.cos;
        arc.p3 = (
            arc.center.0 + arc.radius * sc.cos,
            arc.center.1 + arc.radius * sc.sin,
        );
        arc.angle = anext;
        self.arc_segment(arc, false)
    }

    fn next_arc_quadrant(&mut self, arc: &mut Arc, anext: f64) -> R {
        arc.p0 = arc.p3;
        let (mut x0, mut y0) = arc.p3;
        if arc.fast == 0 {
            let c = self.gs.ctm;
            let mut scale = 0.0;
            let ok = if c.xy == 0.0 && c.yx == 0.0 {
                scale = f64::from(c.xx.abs());
                scale == f64::from(c.yy.abs())
            } else if c.xx == 0.0 && c.yy == 0.0 {
                scale = f64::from(c.xy.abs());
                scale == f64::from(c.yx.abs())
            } else {
                false
            };
            if ok {
                let scaled_radius = arc.radius * scale;
                arc.quadrant_delta = f2x(scaled_radius * 0.552_284_749_830_793_4);
                arc.fast = 1;
            } else {
                arc.fast = -1;
            }
        }
        let (cx, cy, r) = (arc.center.0, arc.center.1, arc.radius);
        match ((anext as i32) >> 1) & 3 {
            0 => {
                arc.sin = 0.0;
                arc.cos = 1.0;
                x0 = cx + r;
                arc.p3 = (x0, cy);
            }
            1 => {
                arc.sin = 1.0;
                arc.cos = 0.0;
                y0 = cy + r;
                arc.p3 = (cx, y0);
            }
            2 => {
                arc.sin = 0.0;
                arc.cos = -1.0;
                x0 = cx - r;
                arc.p3 = (x0, cy);
            }
            _ => {
                arc.sin = -1.0;
                arc.cos = 0.0;
                y0 = cy - r;
                arc.p3 = (cx, y0);
            }
        }
        arc.pt = (x0, y0);
        arc.angle = anext;
        self.arc_segment(arc, true)
    }

    /// `arc_add`.
    fn arc_segment(&mut self, arc: &Arc, quadrant: bool) -> R {
        let ctm = self.gs.ctm;
        let tf = |x: f64, y: f64| -> R<P> {
            let (dx, dy) = ctm.transform(x, y);
            if !fits(dx) || !fits(dy) {
                return Err(E::LimitCheck.into());
            }
            Ok(P {
                x: f2x_round(dx),
                y: f2x_round(dy),
            })
        };
        let mut p0 = if arc.action == Action::Nothing {
            P::default()
        } else {
            tf(arc.p0.0, arc.p0.1)?
        };
        let pt = tf(arc.pt.0, arc.pt.1)?;
        let p3 = tf(arc.p3.0, arc.p3.1)?;
        if !self.gs.path.position_valid() {
            let s = ctm.transform(arc.p0.0, arc.p0.1);
            self.gs.subpath_start = s;
        }
        match arc.action {
            Action::Nothing => p0 = self.gs.path.position,
            Action::Lineto if self.gs.path.position_valid() => {
                self.gs
                    .path
                    .add_line(p0)
                    .map_err(|_| Sig::Err(E::NoCurrentPoint))?;
            }
            _ => {
                self.gs.path.add_point(p0);
            }
        }
        let fraction;
        let p2;
        if quadrant {
            fraction = 0.552_284_749_830_793_4;
            if arc.fast > 0 {
                let d = arc.quadrant_delta;
                if pt.x != p0.x {
                    p0.x = if pt.x > p0.x { p0.x + d } else { p0.x - d };
                }
                if pt.y != p0.y {
                    p0.y = if pt.y > p0.y { p0.y + d } else { p0.y - d };
                }
                p2 = P {
                    x: if pt.x == p3.x {
                        p3.x
                    } else if pt.x > p3.x {
                        p3.x + d
                    } else {
                        p3.x - d
                    },
                    y: if pt.y == p3.y {
                        p3.y
                    } else if pt.y > p3.y {
                        p3.y + d
                    } else {
                        p3.y - d
                    },
                };
                self.gs
                    .path
                    .add_curve(p0, p2, p3)
                    .map_err(|_| Sig::Err(E::NoCurrentPoint))?;
                return Ok(());
            }
        } else {
            let r = arc.radius;
            let (dx, dy) = (arc.pt.0 - arc.p0.0, arc.pt.1 - arc.p0.1);
            let dist = dx * dx + dy * dy;
            let r2 = r * r;
            fraction = if dist >= r2 * 1.0e8 {
                0.0
            } else {
                (4.0 / 3.0) / (1.0 + libm::sqrt(1.0 + dist / r2))
            };
        }
        p0.x += (f64::from(pt.x - p0.x) * fraction) as i32;
        p0.y += (f64::from(pt.y - p0.y) * fraction) as i32;
        p2 = P {
            x: p3.x + (f64::from(pt.x - p3.x) * fraction) as i32,
            y: p3.y + (f64::from(pt.y - p3.y) * fraction) as i32,
        };
        self.gs
            .path
            .add_curve(p0, p2, p3)
            .map_err(|_| Sig::Err(E::NoCurrentPoint))?;
        Ok(())
    }

    // ------ clipping ------

    /// `gx_cpath_intersect` of the current clip with `path` (`clip`,
    /// `eoclip`, `rectclip`): rule -1 nonzero, 1 even-odd.
    pub fn clip_with(&mut self, path: &Path, rule: i32) {
        let old = self.gs.clip.clone();
        let id = self.next_id();
        let flat = if path.curves > 0 {
            flatten_for_test(path)
        } else {
            path.clone()
        };
        let rect_new = flat.is_rectangular().map(|(a, b, _)| (a, b));
        if let (Some(_), true) = (old.rect, rect_new.is_some() || flat.is_void()) {
            let (oa, ob) = old.inner;
            let mut changed = 0;
            let mut same = 0;
            let (mut na, mut nb);
            if let Some((a, b)) = rect_new {
                // (the fill adjustment, 0.5: any part of a pixel)
                let pr = |x: i32| ((x + 128) >> 8) << 8;
                na = P {
                    x: pr(a.x - 127),
                    y: pr(a.y - 127),
                };
                nb = P {
                    x: pr(b.x + 128),
                    y: pr(b.y + 128),
                };
                if oa.x == na.x {
                    same += 1;
                } else if oa.x > na.x {
                    na.x = oa.x;
                    changed += 1;
                    same += 1;
                }
                if oa.y == na.y {
                    same += 1;
                } else if oa.y > na.y {
                    na.y = oa.y;
                    changed += 1;
                    same += 1;
                }
                if ob.x == nb.x {
                    same += 1;
                } else if ob.x < nb.x {
                    nb.x = ob.x;
                    changed += 1;
                    same += 1;
                }
                if ob.y == nb.y {
                    same += 1;
                } else if ob.y <= nb.y {
                    nb.y = ob.y;
                    changed += 1;
                    same += 1;
                }
                if nb.x < na.x || nb.y < na.y {
                    na = nb;
                    changed = 1;
                }
            } else {
                let p = if path.position_valid() {
                    path.position
                } else {
                    P {
                        x: f2x(f64::from(self.gs.ctm.tx)),
                        y: f2x(f64::from(self.gs.ctm.ty)),
                    }
                };
                na = p;
                nb = p;
                changed = 1;
            }
            if same == 4 {
                return;
            }
            let mut c = Clip::rect(id, na, nb);
            if changed == 0 {
                c.path = path.clone();
                c.path_valid = true;
            }
            self.gs.clip = c;
            return;
        }
        // (the slow way)
        let (ba, bb) = path.bbox();
        let path_valid = old.rect.is_some() && includes(&old.inner, ba, bb);
        let mut c = old.clone();
        c.id = id;
        c.rect = None;
        let pix = |a: P, b: P| {
            (
                P {
                    x: a.x & !255,
                    y: a.y & !255,
                },
                P {
                    x: (b.x + 255) & !255,
                    y: (b.y + 255) & !255,
                },
            )
        };
        let (pa, pb) = pix(ba, bb);
        let (oa, ob) = old.outer;
        c.outer = (
            P {
                x: pa.x.max(oa.x),
                y: pa.y.max(oa.y),
            },
            P {
                x: pb.x.min(ob.x),
                y: pb.y.min(ob.y),
            },
        );
        c.inner = (P { x: 1, y: 1 }, P { x: 0, y: 0 });
        if path_valid {
            c.path = path.clone();
            c.path_valid = true;
            c.rule = rule;
            c.list = None;
        } else {
            let next = match old.list.clone() {
                Some(l) => Some(l),
                None => Some(Rc::new(ClipList {
                    path: old.as_path(),
                    rule: old.rule,
                    next: None,
                })),
            };
            c.path_valid = false;
            c.path = Path::new();
            c.list = Some(Rc::new(ClipList {
                path: path.clone(),
                rule,
                next,
            }));
        }
        self.gs.clip = c;
    }
}

impl Clip {
    /// The clip as a path (`gx_cpath_to_path`): its path, or its
    /// rectangle.
    pub fn as_path(&self) -> Path {
        if self.path_valid {
            return self.path.clone();
        }
        let mut p = Path::new();
        if let Some((a, b)) = self.rect {
            p.add_point(a);
            let _ = p.add_line(P { x: a.x, y: b.y });
            let _ = p.add_line(b);
            let _ = p.add_line(P { x: b.x, y: a.y });
            p.close();
        }
        p
    }
}

fn includes(r: &(P, P), a: P, b: P) -> bool {
    r.0.x <= a.x && b.x <= r.1.x && r.0.y <= a.y && b.y <= r.1.y
}

/// The path with its curves as lines between their ends (enough to tell
/// whether it is a rectangle).
fn flatten_for_test(p: &Path) -> Path {
    let mut q = Path::new();
    for e in p.elements() {
        match e {
            Pe::Move(a) => q.add_point(a),
            Pe::Line(a) => {
                let _ = q.add_line(a);
            }
            Pe::Curve(a, b, c) => {
                let _ = q.add_line(a);
                let _ = q.add_line(b);
                let _ = q.add_line(c);
            }
            Pe::Close(_) => q.close(),
        }
    }
    q
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Nothing,
    Moveto,
    Lineto,
}

struct Arc {
    radius: f64,
    center: (f64, f64),
    action: Action,
    p0: (f64, f64),
    p3: (f64, f64),
    pt: (f64, f64),
    sin: f64,
    cos: f64,
    angle: f64,
    fast: i32,
    quadrant_delta: i32,
}

// ------ operators ------

fn op_gsave(ps: &mut Ps<'_>) -> R {
    let g = ps.gs.clone();
    ps.gstack.push(g);
    Ok(())
}
fn op_grestore(ps: &mut Ps<'_>) -> R {
    // (not past the gstate a save made)
    let floor = ps.save_gdepth.last().map_or(0, |&(_, d)| d + 1);
    if ps.gstack.len() > floor {
        let g = ps.gstack.pop().unwrap_or_default();
        set_gstate(ps, g);
    } else if let Some(g) = ps.gstack.get(floor.wrapping_sub(1)).cloned() {
        if floor > 0 {
            set_gstate(ps, g);
        }
    }
    Ok(())
}
fn op_grestoreall(ps: &mut Ps<'_>) -> R {
    let floor = ps.save_gdepth.last().map_or(0, |&(_, d)| d + 1);
    if ps.gstack.len() > floor {
        let g = ps.gstack[floor.min(ps.gstack.len() - 1)].clone();
        ps.gstack.truncate(floor);
        set_gstate(ps, g);
    }
    Ok(())
}
fn op_initgraphics(ps: &mut Ps<'_>) -> R {
    ps.initgraphics();
    Ok(())
}
fn op_gstate(ps: &mut Ps<'_>) -> R {
    let g = ps.gs.clone();
    ps.push(Obj {
        v: V::GState(Rc::new(core::cell::RefCell::new(g))),
        x: false,
    });
    Ok(())
}
fn op_currentgstate(ps: &mut Ps<'_>) -> R {
    let o = ps.top(0)?.clone();
    let V::GState(g) = &o.v else {
        return Err(E::TypeCheck.into());
    };
    *g.borrow_mut() = ps.gs.clone();
    Ok(())
}
fn op_setgstate(ps: &mut Ps<'_>) -> R {
    let o = ps.top(0)?.clone();
    let V::GState(g) = &o.v else {
        return Err(E::TypeCheck.into());
    };
    let g = g.borrow().clone();
    ps.o.pop();
    set_gstate(ps, g);
    Ok(())
}
fn op_setlinewidth(ps: &mut Ps<'_>) -> R {
    let w = ps.pop_num()?;
    ps.gs.line.half_width = (w.abs() / 2.0) as f32;
    Ok(())
}
fn op_currentlinewidth(ps: &mut Ps<'_>) -> R {
    let w = f64::from(ps.gs.line.half_width) * 2.0;
    ps.pushr(w);
    Ok(())
}
fn op_setlinecap(ps: &mut Ps<'_>) -> R {
    let c = ps.int_at(0)?;
    if !(0..=2).contains(&c) {
        return Err(E::RangeCheck.into());
    }
    ps.o.pop();
    ps.gs.line.cap = c as i32;
    Ok(())
}
fn op_currentlinecap(ps: &mut Ps<'_>) -> R {
    let c = i64::from(ps.gs.line.cap);
    ps.pushi(c);
    Ok(())
}
fn op_setlinejoin(ps: &mut Ps<'_>) -> R {
    let c = ps.int_at(0)?;
    if !(0..=2).contains(&c) {
        return Err(E::RangeCheck.into());
    }
    ps.o.pop();
    ps.gs.line.join = c as i32;
    Ok(())
}
fn op_currentlinejoin(ps: &mut Ps<'_>) -> R {
    let c = i64::from(ps.gs.line.join);
    ps.pushi(c);
    Ok(())
}
fn op_setmiterlimit(ps: &mut Ps<'_>) -> R {
    let m = ps.num_at(0)?;
    if m < 1.0 {
        return Err(E::RangeCheck.into());
    }
    ps.o.pop();
    ps.gs.line.miter_limit = m as f32;
    Ok(())
}
fn op_currentmiterlimit(ps: &mut Ps<'_>) -> R {
    let m = f64::from(ps.gs.line.miter_limit);
    ps.pushr(m);
    Ok(())
}
fn op_setdash(ps: &mut Ps<'_>) -> R {
    let off = ps.num_at(0)?;
    let a = ps.top(1)?.clone();
    let v = ps.nums_of(&a)?;
    if v.iter().any(|&x| x < 0.0) || (!v.is_empty() && v.iter().all(|&x| x == 0.0)) {
        return Err(E::RangeCheck.into());
    }
    ps.o.truncate(ps.o.len() - 2);
    ps.gs.line.dash = v.iter().map(|&x| x as f32).collect();
    ps.gs.line.dash_offset = off as f32;
    ps.dev.dash_obj = Some(a);
    Ok(())
}
fn op_currentdash(ps: &mut Ps<'_>) -> R {
    let v: Vec<Obj> = ps.gs.line.dash.iter().map(|&x| Obj::real(x)).collect();
    let a = ps.arr_obj(v);
    ps.push(a);
    let o = f64::from(ps.gs.line.dash_offset);
    ps.pushr(o);
    Ok(())
}
fn op_setflat(ps: &mut Ps<'_>) -> R {
    let f = ps.pop_num()?;
    ps.gs.flatness = f.clamp(0.2, 100.0) as f32;
    Ok(())
}
fn op_currentflat(ps: &mut Ps<'_>) -> R {
    let f = f64::from(ps.gs.flatness);
    ps.pushr(f);
    Ok(())
}
fn op_setstrokeadjust(ps: &mut Ps<'_>) -> R {
    ps.gs.stroke_adjust = ps.pop_bool()?;
    Ok(())
}
fn op_currentstrokeadjust(ps: &mut Ps<'_>) -> R {
    let b = ps.gs.stroke_adjust;
    ps.pushb(b);
    Ok(())
}
fn op_setsmoothness(ps: &mut Ps<'_>) -> R {
    let s = ps.pop_num()?;
    ps.gs.smoothness = s.clamp(0.0, 1.0) as f32;
    Ok(())
}
fn op_currentsmoothness(ps: &mut Ps<'_>) -> R {
    let s = f64::from(ps.gs.smoothness);
    ps.pushr(s);
    Ok(())
}

// matrices

/// A matrix operand: a 6-element array of numbers.
pub fn matrix_of(ps: &Ps<'_>, o: &Obj) -> R<M> {
    let v = ps.nums_of(o)?;
    if v.len() != 6 {
        return Err(E::RangeCheck.into());
    }
    Ok(M::from_slice(&v))
}
/// Stores `m` in array `o` (as reals).
pub fn store_matrix(ps: &mut Ps<'_>, o: &Obj, m: &M) -> R {
    let V::Arr(a) = &o.v else {
        return Err(E::TypeCheck.into());
    };
    if a.len != 6 {
        return Err(E::RangeCheck.into());
    }
    ps.vm.note_arr(&a.b);
    let mut d = a.b.data.borrow_mut();
    for (k, x) in m.to_vec().into_iter().enumerate() {
        d[a.off + k] = Obj::real(x);
    }
    Ok(())
}
fn is_matrix(ps: &Ps<'_>, o: &Obj) -> bool {
    matches!(&o.v, V::Arr(a) if a.len == 6) && ps.nums_of(o).is_ok()
}
fn op_matrix(ps: &mut Ps<'_>) -> R {
    let a = ps.arr_obj(IDENT.to_vec().iter().map(|&x| Obj::real(x)).collect());
    ps.push(a);
    Ok(())
}
fn op_initmatrix(ps: &mut Ps<'_>) -> R {
    ps.gs.ctm = ps.dev.default_matrix();
    Ok(())
}
fn op_identmatrix(ps: &mut Ps<'_>) -> R {
    let o = ps.top(0)?.clone();
    store_matrix(ps, &o, &IDENT)
}
fn op_defaultmatrix(ps: &mut Ps<'_>) -> R {
    let o = ps.top(0)?.clone();
    let m = ps.dev.default_matrix();
    store_matrix(ps, &o, &m)
}
fn op_currentmatrix(ps: &mut Ps<'_>) -> R {
    let o = ps.top(0)?.clone();
    let m = ps.gs.ctm;
    store_matrix(ps, &o, &m)
}
fn op_setmatrix(ps: &mut Ps<'_>) -> R {
    let o = ps.top(0)?.clone();
    let m = matrix_of(ps, &o)?;
    ps.o.pop();
    ps.gs.ctm = m;
    Ok(())
}
fn op_translate(ps: &mut Ps<'_>) -> R {
    let t = ps.top(0)?.clone();
    if is_matrix(ps, &t) {
        let ty = ps.num_at(1)?;
        let tx = ps.num_at(2)?;
        store_matrix(ps, &t, &M::translation(tx, ty))?;
        ps.o.truncate(ps.o.len() - 3);
        ps.push(t);
        return Ok(());
    }
    let ty = ps.num_at(0)?;
    let tx = ps.num_at(1)?;
    ps.o.truncate(ps.o.len() - 2);
    ps.gs.ctm = ps.gs.ctm.translate(tx, ty);
    Ok(())
}
fn op_scale(ps: &mut Ps<'_>) -> R {
    let t = ps.top(0)?.clone();
    if is_matrix(ps, &t) {
        let sy = ps.num_at(1)?;
        let sx = ps.num_at(2)?;
        store_matrix(ps, &t, &M::scaling(sx, sy))?;
        ps.o.truncate(ps.o.len() - 3);
        ps.push(t);
        return Ok(());
    }
    let sy = ps.num_at(0)?;
    let sx = ps.num_at(1)?;
    ps.o.truncate(ps.o.len() - 2);
    ps.gs.ctm = ps.gs.ctm.scale(sx, sy);
    Ok(())
}
fn op_rotate(ps: &mut Ps<'_>) -> R {
    let t = ps.top(0)?.clone();
    if is_matrix(ps, &t) {
        let a = ps.num_at(1)?;
        store_matrix(ps, &t, &M::rotation(a))?;
        ps.o.truncate(ps.o.len() - 2);
        ps.push(t);
        return Ok(());
    }
    let a = ps.pop_num()?;
    ps.gs.ctm = ps.gs.ctm.rotate(a);
    Ok(())
}
fn op_concat(ps: &mut Ps<'_>) -> R {
    let o = ps.top(0)?.clone();
    let m = matrix_of(ps, &o)?;
    ps.o.pop();
    ps.gs.ctm = m.mul(&ps.gs.ctm);
    Ok(())
}
fn op_concatmatrix(ps: &mut Ps<'_>) -> R {
    let r = ps.top(0)?.clone();
    let b = ps.top(1)?.clone();
    let a = ps.top(2)?.clone();
    let (ma, mb) = (matrix_of(ps, &a)?, matrix_of(ps, &b)?);
    store_matrix(ps, &r, &ma.mul(&mb))?;
    ps.o.truncate(ps.o.len() - 3);
    ps.push(r);
    Ok(())
}
fn op_invertmatrix(ps: &mut Ps<'_>) -> R {
    let r = ps.top(0)?.clone();
    let a = ps.top(1)?.clone();
    let m = matrix_of(ps, &a)?
        .invert()
        .ok_or(Sig::Err(E::UndefinedResult))?;
    store_matrix(ps, &r, &m)?;
    ps.o.truncate(ps.o.len() - 2);
    ps.push(r);
    Ok(())
}
fn xform_op(ps: &mut Ps<'_>, f: fn(&M, f64, f64) -> Option<(f64, f64)>) -> R {
    let t = ps.top(0)?.clone();
    let (m, n) = if is_matrix(ps, &t) {
        (matrix_of(ps, &t)?, 1)
    } else {
        (ps.gs.ctm, 0)
    };
    let y = ps.num_at(n)?;
    let x = ps.num_at(n + 1)?;
    let (rx, ry) = f(&m, x, y).ok_or(Sig::Err(E::UndefinedResult))?;
    ps.o.truncate(ps.o.len() - 2 - n);
    ps.pushr(rx);
    ps.pushr(ry);
    Ok(())
}
fn op_transform(ps: &mut Ps<'_>) -> R {
    xform_op(ps, |m, x, y| Some(m.transform(x, y)))
}
fn op_dtransform(ps: &mut Ps<'_>) -> R {
    xform_op(ps, |m, x, y| Some(m.dtransform(x, y)))
}
fn op_itransform(ps: &mut Ps<'_>) -> R {
    xform_op(ps, M::itransform)
}
fn op_idtransform(ps: &mut Ps<'_>) -> R {
    xform_op(ps, M::idtransform)
}

// paths

fn op_newpath(ps: &mut Ps<'_>) -> R {
    ps.newpath();
    Ok(())
}
fn op_currentpoint(ps: &mut Ps<'_>) -> R {
    let (x, y) = ps.currentpoint()?;
    ps.pushr(x);
    ps.pushr(y);
    Ok(())
}
fn op_moveto(ps: &mut Ps<'_>) -> R {
    let y = ps.num_at(0)?;
    let x = ps.num_at(1)?;
    let (dx, dy) = ps.gs.ctm.transform(x, y);
    ps.moveto_dev(dx, dy)?;
    ps.o.truncate(ps.o.len() - 2);
    Ok(())
}
fn op_rmoveto(ps: &mut Ps<'_>) -> R {
    let y = ps.num_at(0)?;
    let x = ps.num_at(1)?;
    let (cx, cy) = ps.gs.cur.ok_or(Sig::Err(E::NoCurrentPoint))?;
    let (dx, dy) = ps.gs.ctm.dtransform(x, y);
    ps.moveto_dev(dx + cx, dy + cy)?;
    ps.o.truncate(ps.o.len() - 2);
    Ok(())
}
fn op_lineto(ps: &mut Ps<'_>) -> R {
    let y = ps.num_at(0)?;
    let x = ps.num_at(1)?;
    if ps.gs.cur.is_none() && !ps.gs.path.position_valid() {
        return Err(E::NoCurrentPoint.into());
    }
    let (dx, dy) = ps.gs.ctm.transform(x, y);
    ps.lineto_dev(dx, dy)?;
    ps.o.truncate(ps.o.len() - 2);
    Ok(())
}
fn op_rlineto(ps: &mut Ps<'_>) -> R {
    let y = ps.num_at(0)?;
    let x = ps.num_at(1)?;
    let (cx, cy) = ps.gs.cur.ok_or(Sig::Err(E::NoCurrentPoint))?;
    let (dx, dy) = ps.gs.ctm.dtransform(x, y);
    ps.lineto_dev(dx + cx, dy + cy)?;
    ps.o.truncate(ps.o.len() - 2);
    Ok(())
}
fn op_curveto(ps: &mut Ps<'_>) -> R {
    let mut c = [0.0; 6];
    for (k, v) in c.iter_mut().enumerate() {
        *v = ps.num_at(5 - k)?;
    }
    if ps.gs.cur.is_none() {
        return Err(E::NoCurrentPoint.into());
    }
    let m = ps.gs.ctm;
    let (a, b) = m.transform(c[0], c[1]);
    let (d, e) = m.transform(c[2], c[3]);
    let (f, g) = m.transform(c[4], c[5]);
    ps.curveto_dev([a, b, d, e, f, g])?;
    ps.o.truncate(ps.o.len() - 6);
    Ok(())
}
fn op_rcurveto(ps: &mut Ps<'_>) -> R {
    let mut c = [0.0; 6];
    for (k, v) in c.iter_mut().enumerate() {
        *v = ps.num_at(5 - k)?;
    }
    let (cx, cy) = ps.gs.cur.ok_or(Sig::Err(E::NoCurrentPoint))?;
    let m = ps.gs.ctm;
    let (a, b) = m.dtransform(c[0], c[1]);
    let (d, e) = m.dtransform(c[2], c[3]);
    let (f, g) = m.dtransform(c[4], c[5]);
    ps.curveto_dev([a + cx, b + cy, d + cx, e + cy, f + cx, g + cy])?;
    ps.o.truncate(ps.o.len() - 6);
    Ok(())
}
fn op_closepath(ps: &mut Ps<'_>) -> R {
    ps.closepath();
    Ok(())
}
fn arc_op(ps: &mut Ps<'_>, cw: bool) -> R {
    let a2 = ps.num_at(0)?;
    let a1 = ps.num_at(1)?;
    let r = ps.num_at(2)?;
    let y = ps.num_at(3)?;
    let x = ps.num_at(4)?;
    ps.arc_add(cw, x, y, r, a1, a2, true)?;
    ps.o.truncate(ps.o.len() - 5);
    Ok(())
}
fn op_arc(ps: &mut Ps<'_>) -> R {
    arc_op(ps, false)
}
fn op_arcn(ps: &mut Ps<'_>) -> R {
    arc_op(ps, true)
}
fn arcto(ps: &mut Ps<'_>, ret: bool) -> R {
    let r = ps.num_at(0)?;
    let y2 = ps.num_at(1)?;
    let x2 = ps.num_at(2)?;
    let y1 = ps.num_at(3)?;
    let x1 = ps.num_at(4)?;
    let (x0, y0) = ps.currentpoint()?;
    let (dx0, dy0) = (x0 - x1, y0 - y1);
    let (dx2, dy2) = (x2 - x1, y2 - y1);
    let sql0 = dx0 * dx0 + dy0 * dy0;
    let sql2 = dx2 * dx2 + dy2 * dy2;
    if sql0 == 0.0 || sql2 == 0.0 {
        return Err(E::UndefinedResult.into());
    }
    let (xt0, yt0, xt2, yt2);
    ps.o.truncate(ps.o.len() - 5);
    if dx0 * dy2 == dy0 * dx2 {
        let (dx, dy) = ps.gs.ctm.transform(x1, y1);
        ps.lineto_dev(dx, dy)?;
        xt0 = x1;
        xt2 = x1;
        yt0 = y1;
        yt2 = y1;
    } else {
        let num = dy0 * dx2 - dy2 * dx0;
        let denom = libm::sqrt(sql0 * sql2) - (dx0 * dx2 + dy0 * dy2);
        let dist = (r * num / denom).abs();
        let (mut l0, mut l2) = (dist / libm::sqrt(sql0), dist / libm::sqrt(sql2));
        if r < 0.0 {
            l0 = -l0;
            l2 = -l2;
        }
        xt0 = x1 + dx0 * l0;
        yt0 = y1 + dy0 * l0;
        xt2 = x1 + dx2 * l2;
        yt2 = y1 + dy2 * l2;
        let arc = Arc {
            radius: r,
            center: (0.0, 0.0),
            action: Action::Lineto,
            p0: (xt0, yt0),
            p3: (xt2, yt2),
            pt: (x1, y1),
            sin: 0.0,
            cos: 0.0,
            angle: 0.0,
            fast: 0,
            quadrant_delta: 0,
        };
        ps.arc_segment(&arc, false)?;
        // (gx_setcurrentpoint_from_path)
        let p = ps.gs.path.position;
        ps.gs.cur = Some((x2f(p.x), x2f(p.y)));
    }
    if ret {
        for v in [xt0, yt0, xt2, yt2] {
            ps.push(Obj::real(v as f32));
        }
    }
    Ok(())
}
fn op_arcto(ps: &mut Ps<'_>) -> R {
    arcto(ps, true)
}
fn op_arct(ps: &mut Ps<'_>) -> R {
    arcto(ps, false)
}
fn op_pathbbox(ps: &mut Ps<'_>) -> R {
    if ps.gs.path.is_null() {
        return Err(E::NoCurrentPoint.into());
    }
    let (a, b) = ps.gs.path.bbox();
    let inv = ps.gs.ctm;
    let pts = [(a.x, a.y), (a.x, b.y), (b.x, a.y), (b.x, b.y)];
    let mut r = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    for (x, y) in pts {
        let (ux, uy) = inv
            .itransform(x2f(x), x2f(y))
            .ok_or(Sig::Err(E::UndefinedResult))?;
        r[0] = r[0].min(ux);
        r[1] = r[1].min(uy);
        r[2] = r[2].max(ux);
        r[3] = r[3].max(uy);
    }
    for v in r {
        ps.pushr(v);
    }
    Ok(())
}
fn op_flattenpath(ps: &mut Ps<'_>) -> R {
    let p = crate::flatten::flatten(&ps.gs.path, f64::from(ps.gs.flatness));
    ps.gs.path = p;
    Ok(())
}
fn op_reversepath(ps: &mut Ps<'_>) -> R {
    let p = reverse(&ps.gs.path);
    ps.gs.path = p;
    Ok(())
}
fn reverse(p: &Path) -> Path {
    // (each subpath reversed, its closepath kept)
    let mut out = Path::new();
    let mut subs: Vec<Vec<Pe>> = Vec::new();
    for e in p.elements() {
        if let Pe::Move(_) = e {
            subs.push(vec![e]);
        } else if let Some(s) = subs.last_mut() {
            s.push(e);
        }
    }
    for s in subs {
        let closed = matches!(s.last(), Some(Pe::Close(_)));
        let pts: Vec<Pe> = s
            .into_iter()
            .filter(|e| !matches!(e, Pe::Close(_)))
            .collect();
        let Some(last) = pts.last() else { continue };
        let end = match *last {
            Pe::Move(a) | Pe::Line(a) | Pe::Curve(_, _, a) | Pe::Close(a) => a,
        };
        out.add_point(end);
        for i in (1..pts.len()).rev() {
            let prev = match pts[i - 1] {
                Pe::Move(a) | Pe::Line(a) | Pe::Curve(_, _, a) | Pe::Close(a) => a,
            };
            match pts[i] {
                Pe::Line(_) => {
                    let _ = out.add_line(prev);
                }
                Pe::Curve(a, b, _) => {
                    let _ = out.add_curve(b, a, prev);
                }
                _ => {}
            }
        }
        if closed {
            out.close();
        }
    }
    out
}
fn op_clippath(ps: &mut Ps<'_>) -> R {
    let p = ps.gs.clip.as_path();
    ps.gs.path = p;
    let pos = ps.gs.path.position;
    ps.gs.cur = Some((x2f(pos.x), x2f(pos.y)));
    Ok(())
}
fn op_initclip(ps: &mut Ps<'_>) -> R {
    ps.initclip();
    Ok(())
}
fn op_clip(ps: &mut Ps<'_>) -> R {
    let p = ps.gs.path.clone();
    ps.clip_with(&p, -1);
    Ok(())
}
fn op_eoclip(ps: &mut Ps<'_>) -> R {
    let p = ps.gs.path.clone();
    ps.clip_with(&p, 1);
    Ok(())
}
/// Rectangle operands: `x y w h` or a numeric array.
pub fn rect_operands(ps: &mut Ps<'_>) -> R<Vec<[f64; 4]>> {
    let t = ps.top(0)?.clone();
    if let V::Arr(_) = t.v {
        let v = ps.nums_of(&t)?;
        ps.o.pop();
        return Ok(v
            .chunks_exact(4)
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect());
    }
    if let V::Str(_) = t.v {
        return Err(E::TypeCheck.into());
    }
    let h = ps.num_at(0)?;
    let w = ps.num_at(1)?;
    let y = ps.num_at(2)?;
    let x = ps.num_at(3)?;
    ps.o.truncate(ps.o.len() - 4);
    Ok(vec![[x, y, w, h]])
}
/// Appends a rectangle as `gs_rectappend` does (counter-clockwise).
pub fn rect_append(ps: &mut Ps<'_>, r: [f64; 4]) -> R {
    let [x, y, w, h] = r;
    let (mut px, py, mut qx, qy) = (x, y, x + w, y + h);
    if (qx >= px) != (qy >= py) {
        core::mem::swap(&mut px, &mut qx);
    }
    let m = ps.gs.ctm;
    let (a, b) = m.transform(px, py);
    ps.moveto_dev(a, b)?;
    for (u, v) in [(qx, py), (qx, qy), (px, qy)] {
        let (a, b) = m.transform(u, v);
        ps.lineto_dev(a, b)?;
    }
    ps.closepath();
    Ok(())
}
fn op_rectclip(ps: &mut Ps<'_>) -> R {
    let rs = rect_operands(ps)?;
    let saved = (ps.gs.path.clone(), ps.gs.cur, ps.gs.subpath_start);
    ps.newpath();
    for r in rs {
        rect_append(ps, r)?;
    }
    let p = ps.gs.path.clone();
    ps.clip_with(&p, -1);
    ps.newpath();
    let _ = saved;
    Ok(())
}
fn op_pathforall(ps: &mut Ps<'_>) -> R {
    ps.need(4)?;
    let close = ps.pop()?;
    let curve = ps.pop()?;
    let line = ps.pop()?;
    let mv = ps.pop()?;
    let inv = ps.gs.ctm;
    let mut items = Vec::new();
    let uv = |x: i32, y: i32| inv.itransform(x2f(x), x2f(y)).unwrap_or((0.0, 0.0));
    for e in ps.gs.path.elements() {
        let r = |v: f64| Obj::real(v as f32);
        match e {
            Pe::Move(p) => {
                let (x, y) = uv(p.x, p.y);
                items.push(vec![r(x), r(y), Obj::int(0)]);
            }
            Pe::Line(p) => {
                let (x, y) = uv(p.x, p.y);
                items.push(vec![r(x), r(y), Obj::int(1)]);
            }
            Pe::Curve(a, b, c) => {
                let (x1, y1) = uv(a.x, a.y);
                let (x2, y2) = uv(b.x, b.y);
                let (x3, y3) = uv(c.x, c.y);
                items.push(vec![r(x1), r(y1), r(x2), r(y2), r(x3), r(y3), Obj::int(2)]);
            }
            Pe::Close(_) => items.push(vec![Obj::int(3)]),
        }
    }
    ps.e.push(Frame::Each {
        items,
        i: 0,
        procs: vec![mv, line, curve, close],
    });
    Ok(())
}

pub fn register(ps: &mut Ps<'_>) {
    let t: &[(&'static str, crate::interp::OpFn)] = &[
        ("gsave", op_gsave),
        ("grestore", op_grestore),
        ("grestoreall", op_grestoreall),
        ("initgraphics", op_initgraphics),
        ("gstate", op_gstate),
        ("currentgstate", op_currentgstate),
        ("setgstate", op_setgstate),
        ("setlinewidth", op_setlinewidth),
        ("currentlinewidth", op_currentlinewidth),
        ("setlinecap", op_setlinecap),
        ("currentlinecap", op_currentlinecap),
        ("setlinejoin", op_setlinejoin),
        ("currentlinejoin", op_currentlinejoin),
        ("setmiterlimit", op_setmiterlimit),
        ("currentmiterlimit", op_currentmiterlimit),
        ("setdash", op_setdash),
        ("currentdash", op_currentdash),
        ("setflat", op_setflat),
        ("currentflat", op_currentflat),
        ("setstrokeadjust", op_setstrokeadjust),
        ("currentstrokeadjust", op_currentstrokeadjust),
        ("setsmoothness", op_setsmoothness),
        ("currentsmoothness", op_currentsmoothness),
        ("matrix", op_matrix),
        ("initmatrix", op_initmatrix),
        ("identmatrix", op_identmatrix),
        ("defaultmatrix", op_defaultmatrix),
        ("currentmatrix", op_currentmatrix),
        ("setmatrix", op_setmatrix),
        ("translate", op_translate),
        ("scale", op_scale),
        ("rotate", op_rotate),
        ("concat", op_concat),
        ("concatmatrix", op_concatmatrix),
        ("invertmatrix", op_invertmatrix),
        ("transform", op_transform),
        ("dtransform", op_dtransform),
        ("itransform", op_itransform),
        ("idtransform", op_idtransform),
        ("newpath", op_newpath),
        ("currentpoint", op_currentpoint),
        ("moveto", op_moveto),
        ("rmoveto", op_rmoveto),
        ("lineto", op_lineto),
        ("rlineto", op_rlineto),
        ("curveto", op_curveto),
        ("rcurveto", op_rcurveto),
        ("closepath", op_closepath),
        ("arc", op_arc),
        ("arcn", op_arcn),
        ("arcto", op_arcto),
        ("arct", op_arct),
        ("pathbbox", op_pathbbox),
        ("flattenpath", op_flattenpath),
        ("reversepath", op_reversepath),
        ("clippath", op_clippath),
        ("initclip", op_initclip),
        ("clip", op_clip),
        ("eoclip", op_eoclip),
        ("rectclip", op_rectclip),
        ("pathforall", op_pathforall),
    ];
    for &(n, f) in t {
        ps.op(n, f);
    }
}
