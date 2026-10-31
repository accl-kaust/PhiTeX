//! Painting (gspaint.c, gsdps1.c): fill, eofill, stroke, rectfill,
//! rectstroke, as the device receives them.

use crate::gstate::{rect_append, rect_operands};
use crate::interp::{Ps, R};
use crate::path::{P, f2x};

impl Ps<'_> {
    pub fn marking(&self) -> bool {
        !self.dev.page_done && !self.null_device
    }
    /// A mark after the first `showpage`: gs writes a second page.
    pub fn second_page(&mut self) -> R {
        if self.dev.page_done && !self.null_device {
            return Err(self.unsupported("more than one page"));
        }
        Ok(())
    }
    /// `gs_fill` / `gs_eofill`: the device's fill, then newpath.
    pub fn fill(&mut self, eo: bool) -> R {
        self.second_page()?;
        if self.marking() {
            if let Some(p) = self.gs.color.pattern.clone() {
                crate::pattern::before_paint(self, &p)?;
            }
            let path = self.gs.path.clone();
            let gs = self.gs.clone();
            self.dev.fill_path(&gs, &path, eo);
        }
        self.newpath();
        Ok(())
    }
    pub fn stroke(&mut self) -> R {
        self.second_page()?;
        if self.marking() {
            if let Some(p) = self.gs.color.pattern.clone() {
                crate::pattern::before_paint(self, &p)?;
            }
            let path = self.gs.path.clone();
            let gs = self.gs.clone();
            self.dev.stroke_path(&gs, &path);
        }
        self.newpath();
        Ok(())
    }
}

fn op_fill(ps: &mut Ps<'_>) -> R {
    ps.fill(false)
}
fn op_eofill(ps: &mut Ps<'_>) -> R {
    ps.fill(true)
}
fn op_stroke(ps: &mut Ps<'_>) -> R {
    ps.stroke()
}
/// `gs_point_transform2fixed`.
fn transform2fixed(m: &crate::matrix::M, x: f64, y: f64) -> Option<P> {
    let txf = f2x(f64::from(m.tx));
    let tyf = f2x(f64::from(m.ty));
    let fx = |v: f64| -> Option<i32> {
        if crate::path::fits(v) {
            Some(f2x(v))
        } else {
            None
        }
    };
    let (mut px, mut py);
    if m.xy != 0.0 {
        px = fx(y * f64::from(m.yx))?;
        py = fx(x * f64::from(m.xy))?;
        if m.xx != 0.0 {
            px = px.checked_add(fx(x * f64::from(m.xx))?)?;
        }
        if m.yy != 0.0 {
            py = py.checked_add(fx(y * f64::from(m.yy))?)?;
        }
    } else {
        px = fx(x * f64::from(m.xx))?;
        py = fx(y * f64::from(m.yy))?;
        if m.yx != 0.0 {
            px = px.checked_add(fx(y * f64::from(m.yx))?)?;
        }
    }
    Some(P {
        x: px.checked_add(txf)?,
        y: py.checked_add(tyf)?,
    })
}
fn op_rectfill(ps: &mut Ps<'_>) -> R {
    let rs = rect_operands(ps)?;
    let c = ps.gs.ctm;
    let fast = ((c.xy == 0.0 && c.yx == 0.0) || (c.xx == 0.0 && c.yy == 0.0))
        && ps.gs.clip.rect.is_some()
        && ps.gs.color.pattern.is_none();
    let mut slow_from = 0;
    if fast {
        let (ia, ib) = ps.gs.clip.inner;
        if ia.x >= ib.x && ia.y >= ib.y {
            return Ok(());
        }
        let mut done = rs.len();
        for (i, r) in rs.iter().enumerate() {
            let (Some(p), Some(q)) = (
                transform2fixed(&c, r[0], r[1]),
                transform2fixed(&c, r[0] + r[2], r[1] + r[3]),
            ) else {
                done = i;
                break;
            };
            let mut a = P {
                x: p.x.min(q.x),
                y: p.y.min(q.y),
            };
            let mut b = P {
                x: p.x.max(q.x),
                y: p.y.max(q.y),
            };
            a.x = a.x.max(ia.x);
            a.y = a.y.max(ia.y);
            b.x = b.x.min(ib.x);
            b.y = b.y.min(ib.y);
            if a.x <= b.x && a.y <= b.y && ps.marking() {
                let gs = ps.gs.clone();
                ps.dev.fill_rect_hl(&gs, a, b);
            }
        }
        if done == rs.len() {
            return Ok(());
        }
        slow_from = done;
    }
    let saved = (ps.gs.path.clone(), ps.gs.cur, ps.gs.subpath_start);
    let do_save = !ps.gs.path.is_null();
    ps.newpath();
    for r in &rs[slow_from..] {
        rect_append(ps, *r)?;
    }
    ps.fill(false)?;
    if do_save {
        ps.gs.path = saved.0;
        ps.gs.cur = saved.1;
        ps.gs.subpath_start = saved.2;
    }
    Ok(())
}
fn op_rectstroke(ps: &mut Ps<'_>) -> R {
    // (an optional matrix after the rectangles)
    let mut mat = None;
    if let Some(t) = ps.o.last().cloned()
        && let crate::obj::V::Arr(a) = &t.v
        && a.len == 6
        && ps.o.len() >= 2
        && matches!(
            ps.o[ps.o.len() - 2].v,
            crate::obj::V::Arr(_) | crate::obj::V::Int(_) | crate::obj::V::Real(_)
        )
        && !(matches!(
            ps.o[ps.o.len() - 2].v,
            crate::obj::V::Int(_) | crate::obj::V::Real(_)
        ) && ps.o.len() < 5)
    {
        let is_rect_array =
            matches!(ps.o[ps.o.len() - 2].v, crate::obj::V::Arr(_)) || ps.o.len() >= 5;
        if is_rect_array {
            mat = Some(crate::gstate::matrix_of(ps, &t)?);
            ps.o.pop();
        }
    }
    let rs = rect_operands(ps)?;
    let saved = ps.gs.clone();
    ps.newpath();
    for r in rs {
        rect_append(ps, r)?;
    }
    if let Some(m) = mat {
        ps.gs.ctm = m.mul(&ps.gs.ctm);
    }
    ps.stroke()?;
    let f = ps.gs.font.clone();
    ps.gs = saved;
    ps.gs.font = f;
    Ok(())
}
fn op_ufill(ps: &mut Ps<'_>) -> R {
    crate::upath::upath_paint(ps, 0)
}
fn op_ueofill(ps: &mut Ps<'_>) -> R {
    crate::upath::upath_paint(ps, 1)
}
fn op_ustroke(ps: &mut Ps<'_>) -> R {
    crate::upath::upath_paint(ps, 2)
}

pub fn register(ps: &mut Ps<'_>) {
    let t: &[(&'static str, crate::interp::OpFn)] = &[
        ("fill", op_fill),
        ("eofill", op_eofill),
        ("stroke", op_stroke),
        ("rectfill", op_rectfill),
        ("rectstroke", op_rectstroke),
        ("ufill", op_ufill),
        ("ueofill", op_ueofill),
        ("ustroke", op_ustroke),
    ];
    for &(n, f) in t {
        ps.op(n, f);
    }
}
