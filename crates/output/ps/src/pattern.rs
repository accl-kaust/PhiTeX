//! Patterns (`makepattern`, `setpattern`) and shadings (`shfill`).
//! Not yet written as pdfwrite does: a pattern's color paints with its
//! PaintProc run in place, clipped to the painted path (the PDF's
//! /Pattern resource is a known difference, see the crate docs).

use alloc::rc::Rc;

use crate::color::Color;
use crate::interp::{E, OpFn, Ps, R};
use crate::matrix::M;
use crate::obj::{Obj, PDict, V};
use crate::pdf::Device;

/// A pattern instance (`makepattern`'s result).
pub struct Pattern {
    pub dict: PDict,
    pub matrix: M,
    pub id: u64,
}

/// The instance of a pattern dictionary (made by `makepattern`, or
/// made now for a bare prototype).
pub fn instance(ps: &mut Ps<'_>, d: &PDict) -> R<Rc<Pattern>> {
    let matrix = match ps.dget_s(d, "Implementation").map(|o| o.v) {
        Some(V::Arr(a)) if a.len == 6 => crate::gstate::matrix_of(ps, &Obj::arr(a))?,
        _ => ps.gs.ctm,
    };
    let id = ps.next_id();
    Ok(Rc::new(Pattern {
        dict: d.clone(),
        matrix,
        id,
    }))
}

/// Before a fill or stroke with a pattern color.
pub fn before_paint(_ps: &mut Ps<'_>, _p: &Rc<Pattern>) -> R {
    Ok(())
}

/// The pattern color's operators (none yet).
pub fn write_pattern_color(_d: &mut Device, _c: &Color, _stroke: bool) {}

fn op_makepattern(ps: &mut Ps<'_>) -> R {
    let mo = ps.top(0)?.clone();
    let m = crate::gstate::matrix_of(ps, &mo)?;
    let V::Dict(d) = &ps.top(1)?.v else {
        return Err(E::TypeCheck.into());
    };
    let d = d.clone();
    let nd = ps.new_dict(d.data.borrow().len() + 2);
    *nd.data.borrow_mut() = d.data.borrow().clone();
    let full = m.mul(&ps.gs.ctm);
    let a = ps.arr_obj(full.to_vec().iter().map(|&x| Obj::real(x)).collect());
    ps.dput_s(&nd, "Implementation", a);
    ps.o.truncate(ps.o.len() - 2);
    ps.push(Obj::dict(nd));
    Ok(())
}
fn op_shfill(ps: &mut Ps<'_>) -> R {
    ps.pop_dict()?;
    Ok(())
}

pub fn register(ps: &mut Ps<'_>) {
    let t: &[(&'static str, OpFn)] = &[("makepattern", op_makepattern), ("shfill", op_shfill)];
    for &(n, f) in t {
        ps.op(n, f);
    }
}
