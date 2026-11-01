//! User paths (`ufill`, `ueofill`, `ustroke`, `uappend`).

use crate::interp::{E, Ps, R, Sig};
use crate::obj::{Obj, V};

/// Appends user path `o` to the current path (its operators executed).
pub fn uappend(ps: &mut Ps<'_>, o: &Obj) -> R {
    let V::Arr(a) = &o.v else {
        return Err(E::TypeCheck.into());
    };
    let items = a.to_vec();
    for it in items {
        if it.x {
            if let V::Name(n) = it.v {
                let name = ps.names.get(n).to_vec();
                if name == b"setbbox" || name == b"ucache" {
                    if name == b"setbbox" {
                        ps.o.truncate(ps.o.len().saturating_sub(4));
                    }
                    continue;
                }
            }
            ps.call(it)?;
        } else {
            ps.push(it);
        }
    }
    Ok(())
}

/// `ufill` (0), `ueofill` (1), `ustroke` (2).
pub fn upath_paint(ps: &mut Ps<'_>, what: u8) -> R {
    let mut mat = None;
    if what == 2
        && let Some(V::Arr(a)) = ps.o.last().map(|o| o.v.clone())
        && a.len == 6
        && ps.o.len() >= 2
        && !a.to_vec().iter().any(|x| x.x)
        && matches!(ps.o[ps.o.len() - 2].v, V::Arr(_))
    {
        let t = ps.pop()?;
        mat = Some(crate::gstate::matrix_of(ps, &t)?);
    }
    let up = ps.pop()?;
    let saved = ps.gs.clone();
    ps.newpath();
    uappend(ps, &up)?;
    if let Some(m) = mat {
        ps.gs.ctm = m.mul(&ps.gs.ctm);
    }
    match what {
        0 => ps.fill(false)?,
        1 => ps.fill(true)?,
        _ => ps.stroke()?,
    }
    let f = ps.gs.font.clone();
    ps.gs = saved;
    ps.gs.font = f;
    let _ = Sig::Exit;
    Ok(())
}

/// The numbers of an encoded number string (PLRM 3.14.5: header 149,
/// representation, count, then the numbers).
pub fn num_string(s: &[u8]) -> Option<alloc::vec::Vec<f64>> {
    if s.len() < 4 || s[0] != 149 {
        return None;
    }
    let r = s[1];
    let be = r < 128;
    let rr = r % 128;
    let n = if be {
        u16::from_be_bytes([s[2], s[3]])
    } else {
        u16::from_le_bytes([s[2], s[3]])
    } as usize;
    let mut v = alloc::vec::Vec::with_capacity(n);
    let mut i = 4;
    let u32_at = |i: usize| {
        if be {
            u32::from_be_bytes([s[i], s[i + 1], s[i + 2], s[i + 3]])
        } else {
            u32::from_le_bytes([s[i], s[i + 1], s[i + 2], s[i + 3]])
        }
    };
    for _ in 0..n {
        let x = match rr {
            0..=31 => {
                if i + 4 > s.len() {
                    return None;
                }
                let x = f64::from(u32_at(i) as i32) / f64::from(1u32 << rr);
                i += 4;
                x
            }
            32..=47 => {
                if i + 2 > s.len() {
                    return None;
                }
                let w = if be {
                    i16::from_be_bytes([s[i], s[i + 1]])
                } else {
                    i16::from_le_bytes([s[i], s[i + 1]])
                };
                i += 2;
                f64::from(w) / f64::from(1u32 << (rr - 32))
            }
            48 | 49 => {
                if i + 4 > s.len() {
                    return None;
                }
                let x = f64::from(f32::from_bits(u32_at(i)));
                i += 4;
                x
            }
            _ => return None,
        };
        v.push(x);
    }
    Some(v)
}

fn op_uappend(ps: &mut Ps<'_>) -> R {
    let o = ps.pop()?;
    uappend(ps, &o)
}

pub fn register(ps: &mut Ps<'_>) {
    let t: &[(&'static str, crate::interp::OpFn)] = &[("uappend", op_uappend)];
    for &(n, f) in t {
        ps.op(n, f);
    }
}
