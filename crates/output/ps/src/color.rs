//! Color spaces and colors as the client gives them (zcolor.c): the PDF
//! writer writes the client's values (`write_color_unchanged`).

use alloc::rc::Rc;
use alloc::vec;
use alloc::vec::Vec;

use crate::interp::{E, Ps, R, Sig};
use crate::obj::{Obj, V};

/// A color space.
#[derive(Clone)]
pub enum Space {
    Gray,
    Rgb,
    Cmyk,
    /// `[/Indexed base hival lookup]`: the lookup's bytes (a procedure's
    /// results, evaluated).
    Indexed {
        base: Rc<Cs>,
        hival: i32,
        lookup: Vec<u8>,
    },
    /// `[/Separation name alt tint]`.
    Separation {
        name: Vec<u8>,
        alt: Rc<Cs>,
        tint: Obj,
        array: Obj,
    },
    /// `[/DeviceN names alt tint ...]`.
    DeviceN {
        names: Vec<Vec<u8>>,
        alt: Rc<Cs>,
        tint: Obj,
        array: Obj,
    },
    /// `[/Pattern under]` or `/Pattern`.
    Pattern {
        under: Option<Rc<Cs>>,
    },
    /// A CIE-based or ICC space, by its number of components, written as
    /// its device equivalent.
    Cie {
        n: usize,
        array: Obj,
    },
}

/// A color space with its identity.
pub struct Cs {
    pub space: Space,
    pub id: u64,
}

impl Cs {
    pub fn n(&self) -> usize {
        match &self.space {
            Space::Gray | Space::Indexed { .. } | Space::Separation { .. } => 1,
            Space::Rgb => 3,
            Space::Cmyk => 4,
            Space::DeviceN { names, .. } => names.len(),
            Space::Pattern { .. } => 1,
            Space::Cie { n, .. } => *n,
        }
    }
}

/// The current color.
#[derive(Clone)]
pub struct Color {
    pub cs: Rc<Cs>,
    pub vals: Vec<f32>,
    /// A pattern's instance (`setpattern`, a Pattern space's color).
    pub pattern: Option<Rc<crate::pattern::Pattern>>,
}

/// The device spaces' shared instances (gs's ICC manager's defaults: one
/// identity each).
pub fn gray() -> Rc<Cs> {
    Rc::new(Cs {
        space: Space::Gray,
        id: 1,
    })
}
pub fn rgb() -> Rc<Cs> {
    Rc::new(Cs {
        space: Space::Rgb,
        id: 2,
    })
}
pub fn cmyk() -> Rc<Cs> {
    Rc::new(Cs {
        space: Space::Cmyk,
        id: 3,
    })
}

impl Default for Color {
    fn default() -> Color {
        Color {
            cs: gray(),
            vals: vec![0.0],
            pattern: None,
        }
    }
}

impl Color {
    /// `gx_hld_saved_color_equal`: the same space and values.
    pub fn same(&self, o: &Color) -> bool {
        self.cs.id == o.cs.id
            && self.vals == o.vals
            && match (&self.pattern, &o.pattern) {
                (None, None) => true,
                (Some(a), Some(b)) => Rc::ptr_eq(a, b),
                _ => false,
            }
    }
}

fn clamp01(v: f64) -> f32 {
    v.clamp(0.0, 1.0) as f32
}

fn op_setgray(ps: &mut Ps<'_>) -> R {
    let g = ps.pop_num()?;
    ps.gs.color = Color {
        cs: gray(),
        vals: vec![clamp01(g)],
        pattern: None,
    };
    Ok(())
}
fn op_setrgbcolor(ps: &mut Ps<'_>) -> R {
    let b = ps.num_at(0)?;
    let g = ps.num_at(1)?;
    let r = ps.num_at(2)?;
    ps.o.truncate(ps.o.len() - 3);
    ps.gs.color = Color {
        cs: rgb(),
        vals: vec![clamp01(r), clamp01(g), clamp01(b)],
        pattern: None,
    };
    Ok(())
}
fn op_setcmykcolor(ps: &mut Ps<'_>) -> R {
    let k = ps.num_at(0)?;
    let y = ps.num_at(1)?;
    let m = ps.num_at(2)?;
    let c = ps.num_at(3)?;
    ps.o.truncate(ps.o.len() - 4);
    ps.gs.color = Color {
        cs: cmyk(),
        vals: vec![clamp01(c), clamp01(m), clamp01(y), clamp01(k)],
        pattern: None,
    };
    Ok(())
}
/// gs's `hsb2rgb` (zcolor.c), in float.
fn hsb2rgb(h: f32, s: f32, b: f32) -> [f32; 3] {
    if s == 0.0 {
        return [b, b, b];
    }
    let h6 = h * 6.0;
    let i = h6 as i32;
    let f = h6 - i as f32;
    let m = b * (1.0 - s);
    let n = b * (1.0 - s * f);
    let k = b * (1.0 - s * (1.0 - f));
    match i {
        1 => [n, b, m],
        2 => [m, b, k],
        3 => [m, n, b],
        4 => [k, m, b],
        5 => [b, m, n],
        _ => [b, k, m],
    }
}
fn op_sethsbcolor(ps: &mut Ps<'_>) -> R {
    let b = ps.num_at(0)?;
    let s = ps.num_at(1)?;
    let h = ps.num_at(2)?;
    ps.o.truncate(ps.o.len() - 3);
    let v = hsb2rgb(clamp01(h), clamp01(s), clamp01(b));
    ps.gs.color = Color {
        cs: rgb(),
        vals: v.to_vec(),
        pattern: None,
    };
    Ok(())
}
/// The color as device gray, RGB and CMYK (gs's conversions, PLRM 7.2).
pub fn as_rgb(ps: &mut Ps<'_>, c: &Color) -> [f32; 3] {
    match &c.cs.space {
        Space::Gray => [c.vals[0]; 3],
        Space::Rgb => [c.vals[0], c.vals[1], c.vals[2]],
        Space::Cmyk => {
            let k = c.vals[3];
            [
                1.0 - (c.vals[0] + k).min(1.0),
                1.0 - (c.vals[1] + k).min(1.0),
                1.0 - (c.vals[2] + k).min(1.0),
            ]
        }
        Space::Indexed {
            base,
            hival,
            lookup,
        } => {
            let n = base.n();
            let i = (libm::roundf(c.vals[0]) as i32).clamp(0, *hival) as usize;
            let v: Vec<f32> = (0..n)
                .map(|k| f32::from(lookup.get(i * n + k).copied().unwrap_or(0)) / 255.0)
                .collect();
            as_rgb(
                ps,
                &Color {
                    cs: base.clone(),
                    vals: v,
                    pattern: None,
                },
            )
        }
        Space::Separation { alt, tint, .. } | Space::DeviceN { alt, tint, .. } => {
            let alt = alt.clone();
            let tint = tint.clone();
            let v =
                tint_transform(ps, &tint, &c.vals, alt.n()).unwrap_or_else(|_| vec![0.0; alt.n()]);
            as_rgb(
                ps,
                &Color {
                    cs: alt,
                    vals: v,
                    pattern: None,
                },
            )
        }
        Space::Cie { n, .. } => match n {
            1 => [c.vals[0]; 3],
            4 => as_rgb(
                ps,
                &Color {
                    cs: cmyk(),
                    vals: c.vals.clone(),
                    pattern: None,
                },
            ),
            _ => [
                c.vals[0],
                *c.vals.get(1).unwrap_or(&0.0),
                *c.vals.get(2).unwrap_or(&0.0),
            ],
        },
        Space::Pattern { .. } => [0.0; 3],
    }
}
/// Runs a tint transform on `vals`, `n` results.
pub fn tint_transform(ps: &mut Ps<'_>, tint: &Obj, vals: &[f32], n: usize) -> R<Vec<f32>> {
    for &v in vals {
        ps.push(Obj::real(v));
    }
    ps.call(tint.clone())?;
    let mut out = vec![0.0; n];
    for k in (0..n).rev() {
        out[k] = ps.pop_num()? as f32;
    }
    Ok(out)
}
fn op_currentgray(ps: &mut Ps<'_>) -> R {
    let c = ps.gs.color.clone();
    let g = match c.cs.space {
        Space::Gray => c.vals[0],
        Space::Cmyk => {
            1.0 - (0.3 * c.vals[0] + 0.59 * c.vals[1] + 0.11 * c.vals[2] + c.vals[3]).min(1.0)
        }
        _ => {
            let [r, g, b] = as_rgb(ps, &c);
            0.3 * r + 0.59 * g + 0.11 * b
        }
    };
    ps.push(Obj::real(g));
    Ok(())
}
fn op_currentrgbcolor(ps: &mut Ps<'_>) -> R {
    let c = ps.gs.color.clone();
    for v in as_rgb(ps, &c) {
        ps.push(Obj::real(v));
    }
    Ok(())
}
fn op_currentcmykcolor(ps: &mut Ps<'_>) -> R {
    let c = ps.gs.color.clone();
    let v = match c.cs.space {
        Space::Cmyk => [c.vals[0], c.vals[1], c.vals[2], c.vals[3]],
        Space::Gray => [0.0, 0.0, 0.0, 1.0 - c.vals[0]],
        _ => {
            let [r, g, b] = as_rgb(ps, &c);
            let (cc, m, y) = (1.0 - r, 1.0 - g, 1.0 - b);
            let k = cc.min(m).min(y);
            [cc - k, m - k, y - k, k]
        }
    };
    for x in v {
        ps.push(Obj::real(x));
    }
    Ok(())
}
fn op_currenthsbcolor(ps: &mut Ps<'_>) -> R {
    let c = ps.gs.color.clone();
    let [r, g, b] = as_rgb(ps, &c);
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let d = mx - mn;
    let s = if mx == 0.0 { 0.0 } else { d / mx };
    let h = if d == 0.0 {
        0.0
    } else {
        let h = if r == mx {
            (g - b) / d
        } else if g == mx {
            2.0 + (b - r) / d
        } else {
            4.0 + (r - g) / d
        } / 6.0;
        if h < 0.0 { h + 1.0 } else { h }
    };
    for x in [h, s, mx] {
        ps.push(Obj::real(x));
    }
    Ok(())
}

/// A color space from its operand (a name or an array).
pub fn space_of(ps: &mut Ps<'_>, o: &Obj) -> R<Rc<Cs>> {
    let (name, arr) = match &o.v {
        V::Name(n) => (ps.names.get(*n).to_vec(), None),
        V::Arr(a) => {
            if a.len == 0 {
                return Err(E::RangeCheck.into());
            }
            match a.get(0).v {
                V::Name(n) => (ps.names.get(n).to_vec(), Some(a.clone())),
                _ => return Err(E::TypeCheck.into()),
            }
        }
        _ => return Err(E::TypeCheck.into()),
    };
    let id = ps.next_id() + 100;
    let space = match name.as_slice() {
        b"DeviceGray" | b"G" => return Ok(gray()),
        b"DeviceRGB" | b"RGB" => return Ok(rgb()),
        b"DeviceCMYK" | b"CMYK" => return Ok(cmyk()),
        b"Pattern" => {
            let under = match &arr {
                Some(a) if a.len > 1 => Some(space_of(ps, &a.get(1))?),
                _ => None,
            };
            Space::Pattern { under }
        }
        b"Indexed" | b"I" => {
            let a = arr.ok_or(Sig::Err(E::RangeCheck))?;
            if a.len < 4 {
                return Err(E::RangeCheck.into());
            }
            let base = space_of(ps, &a.get(1))?;
            let hival = match a.get(2).v {
                V::Int(i) => i as i32,
                _ => return Err(E::TypeCheck.into()),
            };
            let lk = a.get(3);
            let n = base.n();
            let lookup = match &lk.v {
                V::Str(s) => s.bytes(),
                V::Arr(_) => {
                    let mut v = Vec::new();
                    for i in 0..=hival {
                        ps.push(Obj::int(i64::from(i)));
                        ps.call(lk.clone())?;
                        let mut c = vec![0u8; n];
                        for k in (0..n).rev() {
                            c[k] = (ps.pop_num()? * 255.0 + 0.5) as u8;
                        }
                        v.extend(c);
                    }
                    v
                }
                _ => return Err(E::TypeCheck.into()),
            };
            Space::Indexed {
                base,
                hival,
                lookup,
            }
        }
        b"Separation" => {
            let a = arr.ok_or(Sig::Err(E::RangeCheck))?;
            if a.len < 4 {
                return Err(E::RangeCheck.into());
            }
            let name = match &a.get(1).v {
                V::Name(n) => ps.names.get(*n).to_vec(),
                V::Str(s) => s.bytes(),
                _ => return Err(E::TypeCheck.into()),
            };
            let alt = space_of(ps, &a.get(2))?;
            Space::Separation {
                name,
                alt,
                tint: a.get(3),
                array: o.clone(),
            }
        }
        b"DeviceN" => {
            let a = arr.ok_or(Sig::Err(E::RangeCheck))?;
            if a.len < 4 {
                return Err(E::RangeCheck.into());
            }
            let names = match &a.get(1).v {
                V::Arr(n) => n
                    .to_vec()
                    .iter()
                    .map(|x| match &x.v {
                        V::Name(n) => ps.names.get(*n).to_vec(),
                        V::Str(s) => s.bytes(),
                        _ => Vec::new(),
                    })
                    .collect(),
                _ => return Err(E::TypeCheck.into()),
            };
            let alt = space_of(ps, &a.get(2))?;
            Space::DeviceN {
                names,
                alt,
                tint: a.get(3),
                array: o.clone(),
            }
        }
        b"CIEBasedA" | b"CalGray" => Space::Cie {
            n: 1,
            array: o.clone(),
        },
        b"CIEBasedABC" | b"CalRGB" | b"Lab" | b"CIEBasedDEF" => Space::Cie {
            n: 3,
            array: o.clone(),
        },
        b"CIEBasedDEFG" => Space::Cie {
            n: 4,
            array: o.clone(),
        },
        b"ICCBased" => {
            let n = match &arr {
                Some(a) if a.len > 1 => match &a.get(1).v {
                    V::Dict(d) => ps.dget_s(d, "N").and_then(|x| x.num()).unwrap_or(3.0) as usize,
                    _ => 3,
                },
                _ => 3,
            };
            Space::Cie {
                n,
                array: o.clone(),
            }
        }
        _ => return Err(E::Undefined.into()),
    };
    Ok(Rc::new(Cs { space, id }))
}

/// The initial color of a space (`setcolorspace`).
pub fn initial(cs: &Rc<Cs>) -> Vec<f32> {
    match &cs.space {
        Space::Cmyk => vec![0.0, 0.0, 0.0, 1.0],
        Space::Separation { .. } | Space::DeviceN { .. } => vec![1.0; cs.n()],
        Space::Pattern { .. } => vec![],
        _ => vec![0.0; cs.n()],
    }
}

fn op_setcolorspace(ps: &mut Ps<'_>) -> R {
    let o = ps.top(0)?.clone();
    let cs = space_of(ps, &o)?;
    ps.o.pop();
    let vals = initial(&cs);
    ps.gs.color = Color {
        cs,
        vals,
        pattern: None,
    };
    ps.dev.cs_obj = Some(o);
    Ok(())
}
fn op_currentcolorspace(ps: &mut Ps<'_>) -> R {
    let o = match &ps.gs.color.cs.space {
        Space::Gray => {
            let n = ps.nobj("DeviceGray");
            ps.arr_obj(vec![n])
        }
        Space::Rgb => {
            let n = ps.nobj("DeviceRGB");
            ps.arr_obj(vec![n])
        }
        Space::Cmyk => {
            let n = ps.nobj("DeviceCMYK");
            ps.arr_obj(vec![n])
        }
        _ => match ps.dev.cs_obj.clone() {
            Some(o @ Obj { v: V::Arr(_), .. }) => o,
            Some(o) => ps.arr_obj(vec![o]),
            None => {
                let n = ps.nobj("DeviceGray");
                ps.arr_obj(vec![n])
            }
        },
    };
    ps.push(o);
    Ok(())
}
fn op_setcolor(ps: &mut Ps<'_>) -> R {
    let cs = ps.gs.color.cs.clone();
    if let Space::Pattern { under } = &cs.space {
        let p = ps.pop_dict()?;
        let pat = crate::pattern::instance(ps, &p)?;
        let n = under.as_ref().map_or(0, |u| u.n());
        let mut vals = vec![0.0; n];
        for k in (0..n).rev() {
            vals[k] = ps.pop_num()? as f32;
        }
        ps.gs.color = Color {
            cs,
            vals,
            pattern: Some(pat),
        };
        return Ok(());
    }
    let n = cs.n();
    ps.need(n)?;
    let mut vals = vec![0.0; n];
    for k in (0..n).rev() {
        vals[k] = ps.num_at(n - 1 - k)? as f32;
    }
    ps.o.truncate(ps.o.len() - n);
    // (device and separation components clamped to [0, 1])
    let vals = match &cs.space {
        Space::Indexed { hival, .. } => vec![vals[0].clamp(0.0, *hival as f32)],
        Space::Cie { .. } => vals,
        _ => vals.into_iter().map(|v| v.clamp(0.0, 1.0)).collect(),
    };
    ps.gs.color = Color {
        cs,
        vals,
        pattern: None,
    };
    Ok(())
}
fn op_currentcolor(ps: &mut Ps<'_>) -> R {
    let v = ps.gs.color.vals.clone();
    for x in v {
        ps.push(Obj::real(x));
    }
    Ok(())
}
fn op_setpattern(ps: &mut Ps<'_>) -> R {
    let d = match &ps.top(0)?.v {
        V::Dict(d) => d.clone(),
        _ => return Err(E::TypeCheck.into()),
    };
    // (an uncolored pattern takes the current color as its underlying)
    let paint = ps
        .dget_s(&d, "PaintType")
        .and_then(|o| o.num())
        .unwrap_or(1.0);
    let under = (paint == 2.0).then(|| ps.gs.color.cs.clone());
    let id = ps.next_id() + 100;
    let cs = Rc::new(Cs {
        space: Space::Pattern {
            under: under.clone(),
        },
        id,
    });
    let vals = if under.is_some() {
        ps.gs.color.vals.clone()
    } else {
        Vec::new()
    };
    ps.o.pop();
    let pat = crate::pattern::instance(ps, &d)?;
    ps.gs.color = Color {
        cs,
        vals,
        pattern: Some(pat),
    };
    Ok(())
}
fn op_setoverprint(ps: &mut Ps<'_>) -> R {
    ps.gs.overprint = ps.pop_bool()?;
    Ok(())
}
fn op_currentoverprint(ps: &mut Ps<'_>) -> R {
    let b = ps.gs.overprint;
    ps.pushb(b);
    Ok(())
}
fn op_settransfer(ps: &mut Ps<'_>) -> R {
    let p = ps.pop()?;
    ps.gs.transfer = Some(p);
    Ok(())
}
fn op_currenttransfer(ps: &mut Ps<'_>) -> R {
    let t = ps
        .gs
        .transfer
        .clone()
        .unwrap_or_else(|| ps.arr_obj(Vec::new()).exec());
    ps.push(t);
    Ok(())
}
fn op_setcolortransfer(ps: &mut Ps<'_>) -> R {
    ps.need(4)?;
    ps.o.truncate(ps.o.len() - 4);
    Ok(())
}
fn op_currentcolortransfer(ps: &mut Ps<'_>) -> R {
    for _ in 0..4 {
        let p = ps.arr_obj(Vec::new()).exec();
        ps.push(p);
    }
    Ok(())
}
fn op_setscreen(ps: &mut Ps<'_>) -> R {
    ps.need(3)?;
    ps.o.truncate(ps.o.len() - 3);
    Ok(())
}
fn op_currentscreen(ps: &mut Ps<'_>) -> R {
    ps.pushr(60.0);
    ps.pushr(45.0);
    let p = ps.arr_obj(Vec::new()).exec();
    ps.push(p);
    Ok(())
}
fn op_setcolorscreen(ps: &mut Ps<'_>) -> R {
    ps.need(12)?;
    ps.o.truncate(ps.o.len() - 12);
    Ok(())
}
fn op_sethalftone(ps: &mut Ps<'_>) -> R {
    ps.pop_dict().map(|_| ())
}
fn op_currenthalftone(ps: &mut Ps<'_>) -> R {
    let d = ps.new_dict(4);
    ps.dput_s(&d, "HalftoneType", Obj::int(1));
    ps.dput_s(&d, "Frequency", Obj::real(60.0));
    ps.dput_s(&d, "Angle", Obj::real(45.0));
    ps.push(Obj::dict(d));
    Ok(())
}
fn op_setblackgeneration(ps: &mut Ps<'_>) -> R {
    ps.pop().map(|_| ())
}
fn op_currentblackgeneration(ps: &mut Ps<'_>) -> R {
    let p = ps.arr_obj(Vec::new()).exec();
    ps.push(p);
    Ok(())
}
fn op_setcolorrendering(ps: &mut Ps<'_>) -> R {
    ps.pop().map(|_| ())
}
fn op_currentcolorrendering(ps: &mut Ps<'_>) -> R {
    let d = ps.new_dict(4);
    ps.push(Obj::dict(d));
    Ok(())
}
fn op_setrenderingintent(ps: &mut Ps<'_>) -> R {
    ps.pop().map(|_| ())
}

pub fn register(ps: &mut Ps<'_>) {
    let t: &[(&'static str, crate::interp::OpFn)] = &[
        ("setgray", op_setgray),
        ("setrgbcolor", op_setrgbcolor),
        ("setcmykcolor", op_setcmykcolor),
        ("sethsbcolor", op_sethsbcolor),
        ("currentgray", op_currentgray),
        ("currentrgbcolor", op_currentrgbcolor),
        ("currentcmykcolor", op_currentcmykcolor),
        ("currenthsbcolor", op_currenthsbcolor),
        ("setcolorspace", op_setcolorspace),
        ("currentcolorspace", op_currentcolorspace),
        ("setcolor", op_setcolor),
        ("currentcolor", op_currentcolor),
        ("setpattern", op_setpattern),
        ("setoverprint", op_setoverprint),
        ("currentoverprint", op_currentoverprint),
        ("settransfer", op_settransfer),
        ("currenttransfer", op_currenttransfer),
        ("setcolortransfer", op_setcolortransfer),
        ("currentcolortransfer", op_currentcolortransfer),
        ("setscreen", op_setscreen),
        ("currentscreen", op_currentscreen),
        ("setcolorscreen", op_setcolorscreen),
        ("currentcolorscreen", op_currentscreen),
        ("sethalftone", op_sethalftone),
        ("currenthalftone", op_currenthalftone),
        ("setblackgeneration", op_setblackgeneration),
        ("currentblackgeneration", op_currentblackgeneration),
        ("setundercolorremoval", op_setblackgeneration),
        ("currentundercolorremoval", op_currentblackgeneration),
        ("setcolorrendering", op_setcolorrendering),
        ("currentcolorrendering", op_currentcolorrendering),
        (".setrenderingintent", op_setrenderingintent),
    ];
    for &(n, f) in t {
        ps.op(n, f);
    }
}
