//! Images (`image`, `colorimage`, `imagemask`): their data read from
//! strings, procedures or files as the operators define, and written as
//! image XObjects; color spaces as resources.

use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

use crate::color::{Cs, Space};
use crate::interp::{E, OpFn, Ps, R, Sig};
use crate::matrix::M;
use crate::obj::{Obj, PDict, V};
use crate::pdf::{Ctx, Device};

#[derive(Default)]
pub struct Images {
    /// Color space resources written: (space id, object).
    pub spaces: Vec<(u64, i64)>,
}

/// A color space's PDF object.
pub fn space_body(d: &mut Device, cs: &Cs) -> String {
    match &cs.space {
        Space::Gray => "/DeviceGray".into(),
        Space::Rgb => "/DeviceRGB".into(),
        Space::Cmyk => "/DeviceCMYK".into(),
        Space::Cie { n, .. } => match n {
            1 => "/DeviceGray".into(),
            4 => "/DeviceCMYK".into(),
            _ => "/DeviceRGB".into(),
        },
        Space::Indexed {
            base,
            hival,
            lookup,
        } => {
            let b = space_body(d, base);
            let mut s = format!("[/Indexed {b} {hival} <");
            for c in lookup {
                s.push_str(&format!("{c:02x}"));
            }
            s.push_str(">]");
            s
        }
        Space::Separation { name, alt, .. } => {
            let b = space_body(d, alt);
            format!("[/Separation/{} {b} {{}}]", String::from_utf8_lossy(name))
        }
        Space::DeviceN { names, alt, .. } => {
            let b = space_body(d, alt);
            let mut s = String::from("[/DeviceN[");
            for n in names {
                s.push('/');
                s.push_str(&String::from_utf8_lossy(n));
            }
            s.push_str(&format!("] {b} {{}}]"));
            s
        }
        Space::Pattern { .. } => "/Pattern".into(),
    }
}

/// Where image data comes from.
enum Src {
    Str(Vec<u8>),
    Proc(Obj),
    File(crate::obj::PFile),
}

impl Ps<'_> {
    /// Reads `n` bytes of image data from the sources (one per
    /// component when `multi`).
    fn image_data(&mut self, srcs: &[Src], n: usize) -> R<Vec<Vec<u8>>> {
        let mut out: Vec<Vec<u8>> = srcs.iter().map(|_| Vec::new()).collect();
        for (k, s) in srcs.iter().enumerate() {
            match s {
                Src::Str(b) => {
                    while out[k].len() < n {
                        if b.is_empty() {
                            break;
                        }
                        out[k].extend_from_slice(b);
                    }
                }
                Src::File(f) => {
                    let mut fb = f.borrow_mut();
                    let rest = fb.rest();
                    let m = rest.len().min(n);
                    out[k].extend_from_slice(&rest[..m]);
                    fb.pos += m;
                }
                Src::Proc(_) => {}
            }
        }
        // (procedures: called in turn until each has given its share)
        let procs: Vec<(usize, Obj)> = srcs
            .iter()
            .enumerate()
            .filter_map(|(k, s)| {
                if let Src::Proc(p) = s {
                    Some((k, p.clone()))
                } else {
                    None
                }
            })
            .collect();
        if !procs.is_empty() {
            loop {
                if procs.iter().all(|(k, _)| out[*k].len() >= n) {
                    break;
                }
                let mut empty = false;
                for (k, p) in &procs {
                    let depth = self.o.len();
                    self.call(p.clone())?;
                    let r = if self.o.len() > depth {
                        self.pop()?
                    } else {
                        Obj::NULL
                    };
                    self.o.truncate(depth);
                    match r.v {
                        V::Str(s) => {
                            if s.len == 0 {
                                empty = true;
                            }
                            out[*k].extend_from_slice(&s.bytes());
                        }
                        _ => return Err(E::TypeCheck.into()),
                    }
                }
                if empty {
                    break;
                }
            }
        }
        for o in &mut out {
            o.truncate(n);
        }
        Ok(out)
    }

    fn src_of(&mut self, o: &Obj) -> R<Src> {
        Ok(match &o.v {
            V::Str(s) => Src::Str(s.bytes()),
            V::File(f) => Src::File(f.clone()),
            V::Arr(_) if o.x => Src::Proc(o.clone()),
            _ => return Err(E::TypeCheck.into()),
        })
    }

    /// Paints an image: `ncomp` components of `bpc` bits, `mask` for
    /// imagemask (`polarity` its Decode [1 0]).
    fn paint_image(
        &mut self,
        w: i64,
        h: i64,
        bpc: i64,
        mat: M,
        srcs: Vec<Src>,
        ncomp: usize,
        mask: Option<bool>,
        cs: Option<Rc<Cs>>,
    ) -> R {
        if w <= 0 || h <= 0 {
            return Ok(());
        }
        let multi = srcs.len() > 1;
        let row_bits = if multi {
            w * bpc
        } else {
            w * bpc * ncomp as i64
        };
        let n = ((row_bits + 7) / 8 * h) as usize;
        let data = self.image_data(&srcs, n)?;
        if !self.marking() {
            return Ok(());
        }
        // (interleave separate sources)
        let samples: Vec<u8> = if multi && bpc == 8 {
            let mut v = Vec::with_capacity(n * ncomp);
            for i in 0..n {
                for d in &data {
                    v.push(d.get(i).copied().unwrap_or(0));
                }
            }
            v
        } else {
            data.into_iter().next().unwrap_or_default()
        };
        let gs = self.gs.clone();
        if !self.dev.prepare_fill_with_clip(&gs, true) {
            return Ok(());
        }
        if mask.is_some() {
            self.dev.set_color(&gs.color, false);
        }
        let im = mat.invert().ok_or(Sig::Err(E::UndefinedResult))?;
        let m = M {
            xx: 1.0 / w as f32,
            xy: 0.0,
            yx: 0.0,
            yy: -1.0 / h as f32,
            tx: 0.0,
            ty: 1.0,
        }
        .mul(&im)
        .mul(&gs.ctm);
        let _ = m;
        let full = im.mul(&gs.ctm);
        let um = M {
            xx: w as f32,
            xy: 0.0,
            yx: 0.0,
            yy: -(h as f32),
            tx: 0.0,
            ty: h as f32,
        }
        .mul(&full);
        let id = self.dev.alloc();
        let z = phitex_codecs::deflate::compress(&samples, 6);
        let mut dict = format!("/Subtype/Image\n/Width {w}\n/Height {h}\n");
        match mask {
            Some(pol) => {
                dict.push_str("/ImageMask true\n/BitsPerComponent 1\n");
                if pol {
                    dict.push_str("/Decode[1 0]\n");
                }
            }
            None => {
                let c = cs.unwrap_or_else(crate::color::gray);
                let b = space_body(&mut self.dev, &c);
                dict.push_str(&format!(
                    "/ColorSpace{}{b}\n/BitsPerComponent {bpc}\n",
                    if b.starts_with('/') { "" } else { " " }
                ));
            }
        }
        dict.push_str("/Filter/FlateDecode");
        let mut o = format!(
            "{id} 0 obj\n<</Type/XObject{dict}/Length {}>>stream\n",
            z.len()
        )
        .into_bytes();
        o.extend_from_slice(&z);
        o.extend_from_slice(b"\nendstream\nendobj\n");
        self.dev.direct.push((id, o));
        self.dev.open_page(Ctx::Stream);
        crate::pdf::put_matrix(&mut self.dev, "q ", &um, "cm\n");
        self.dev.put(&format!("/R{id} Do\nQ\n"));
        self.dev.add_resource("XObject", id);
        if mask.is_some() {
            self.dev.procset_image_i = true;
        } else if ncomp == 1 {
            self.dev.procset_image_b = true;
        } else {
            self.dev.procset_image_c = true;
        }
        Ok(())
    }

    fn image_dict(&mut self, d: &PDict, mask: bool) -> R {
        let num = |ps: &mut Ps<'_>, k: &str| {
            ps.dget_s(d, k)
                .and_then(|o| o.num())
                .ok_or(Sig::Err(E::RangeCheck))
        };
        let w = num(self, "Width")? as i64;
        let h = num(self, "Height")? as i64;
        let bpc = if mask {
            1
        } else {
            num(self, "BitsPerComponent")? as i64
        };
        let mo = self
            .dget_s(d, "ImageMatrix")
            .ok_or(Sig::Err(E::RangeCheck))?;
        let mat = crate::gstate::matrix_of(self, &mo)?;
        let ds = self
            .dget_s(d, "DataSource")
            .ok_or(Sig::Err(E::RangeCheck))?;
        let multi = self
            .dget_s(d, "MultipleDataSources")
            .is_some_and(|o| matches!(o.v, V::Bool(true)));
        let cs = self.gs.color.cs.clone();
        let ncomp = if mask { 1 } else { cs.n() };
        let srcs = if multi {
            let V::Arr(a) = &ds.v else {
                return Err(E::TypeCheck.into());
            };
            let mut v = Vec::new();
            for o in a.to_vec() {
                v.push(self.src_of(&o)?);
            }
            v
        } else {
            alloc::vec![self.src_of(&ds)?]
        };
        let pol = mask
            && self
                .dget_s(d, "Decode")
                .is_some_and(|o| self.nums_of(&o).is_ok_and(|v| v.first() == Some(&1.0)));
        self.paint_image(
            w,
            h,
            bpc,
            mat,
            srcs,
            ncomp,
            mask.then_some(pol),
            (!mask).then_some(cs),
        )
    }
}

fn op_image(ps: &mut Ps<'_>) -> R {
    if let V::Dict(d) = &ps.top(0)?.v {
        let d = d.clone();
        ps.o.pop();
        return ps.image_dict(&d, false);
    }
    ps.need(5)?;
    let src = ps.top(0)?.clone();
    let mo = ps.top(1)?.clone();
    let bpc = ps.int_at(2)?;
    let h = ps.int_at(3)?;
    let w = ps.int_at(4)?;
    let mat = crate::gstate::matrix_of(ps, &mo)?;
    let s = ps.src_of(&src)?;
    ps.o.truncate(ps.o.len() - 5);
    ps.paint_image(
        w,
        h,
        bpc,
        mat,
        alloc::vec![s],
        1,
        None,
        Some(crate::color::gray()),
    )
}
fn op_imagemask(ps: &mut Ps<'_>) -> R {
    if let V::Dict(d) = &ps.top(0)?.v {
        let d = d.clone();
        ps.o.pop();
        return ps.image_dict(&d, true);
    }
    ps.need(5)?;
    let src = ps.top(0)?.clone();
    let mo = ps.top(1)?.clone();
    let pol = matches!(ps.top(2)?.v, V::Bool(true));
    let h = ps.int_at(3)?;
    let w = ps.int_at(4)?;
    let mat = crate::gstate::matrix_of(ps, &mo)?;
    let s = ps.src_of(&src)?;
    ps.o.truncate(ps.o.len() - 5);
    ps.paint_image(w, h, 1, mat, alloc::vec![s], 1, Some(!pol), None)
}
fn op_colorimage(ps: &mut Ps<'_>) -> R {
    ps.need(7)?;
    let ncomp = ps.int_at(0)? as usize;
    let multi = matches!(ps.top(1)?.v, V::Bool(true));
    let nsrc = if multi { ncomp } else { 1 };
    ps.need(6 + nsrc)?;
    let mut srcs = Vec::new();
    for k in (0..nsrc).rev() {
        let o = ps.top(2 + k)?.clone();
        srcs.push(ps.src_of(&o)?);
    }
    let mo = ps.top(2 + nsrc)?.clone();
    let bpc = ps.int_at(3 + nsrc)?;
    let h = ps.int_at(4 + nsrc)?;
    let w = ps.int_at(5 + nsrc)?;
    let mat = crate::gstate::matrix_of(ps, &mo)?;
    ps.o.truncate(ps.o.len() - (6 + nsrc));
    let cs = match ncomp {
        1 => crate::color::gray(),
        4 => crate::color::cmyk(),
        _ => crate::color::rgb(),
    };
    ps.paint_image(w, h, bpc, mat, srcs, ncomp, None, Some(cs))
}

pub fn register(ps: &mut Ps<'_>) {
    let t: &[(&'static str, OpFn)] = &[
        ("image", op_image),
        ("imagemask", op_imagemask),
        ("colorimage", op_colorimage),
    ];
    for &(n, f) in t {
        ps.op(n, f);
    }
}
